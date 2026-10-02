use std::collections::{BTreeSet, HashMap, HashSet};
use std::future::Future;

use riven_format::{
    Entry, EntryFile, Kind, LoaderKind, PackPath, Project, Reason, Side, Source as EntrySource,
    UpdatePolicy,
};
use riven_sources::{DependencyKind, ProjectInfo, Source, Target, Version, loader_name};

use crate::check::{Env, Installed, Problem, check};
use crate::meta::{DepKind, JarMeta, ModDep};
use crate::version::ModVersion;

const METADATA_ROUNDS: usize = 5;
/// Candidate jars downloaded at most to find a version a parent accepts.
const MAX_CANDIDATES: usize = 12;

/// Provides jar bytes for an entry file, e.g. through the content store.
pub trait JarFetcher: Sync {
    fn jar(&self, file: &EntryFile) -> impl Future<Output = Result<Vec<u8>, String>> + Send;
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error(transparent)]
    Source(#[from] riven_sources::Error),
    #[error("`{project}` is already in the pack as `{id}`")]
    AlreadyPresent { project: String, id: String },
    #[error("`{0}` is a modpack, not content")]
    Modpack(String),
    #[error("`{project}` has no version for Minecraft {minecraft} on {loader}")]
    NoCompatibleVersion {
        project: String,
        minecraft: String,
        loader: String,
    },
    #[error("`{project}` has no version `{version}` for Minecraft {minecraft} on {loader}")]
    NoSuchVersion {
        project: String,
        version: String,
        minecraft: String,
        loader: String,
    },
    #[error("version `{0}` has no files")]
    NoFile(String),
    #[error("version `{version}` has an unsafe file name: {source}")]
    BadPath {
        version: String,
        source: riven_format::PathError,
    },
    #[error("no entry `{0}` in the pack")]
    UnknownEntry(String),
}

/// Per-request choices for `riven add`.
#[derive(Debug, Clone, Default)]
pub struct AddOptions {
    /// A specific version (id or version number) instead of the newest.
    pub version: Option<String>,
    pub side: Option<Side>,
    pub group: Option<String>,
    pub pin: bool,
}

/// A previewable change to `riven.json`.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub add: Vec<Entry>,
    /// `(old, new)` pairs sharing an id.
    pub update: Vec<(Entry, Entry)>,
    pub remove: Vec<Entry>,
    /// Problems `riven check` would report for the planned entries.
    pub problems: Vec<Problem>,
    pub notes: Vec<String>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.update.is_empty() && self.remove.is_empty()
    }

    pub fn apply(&self, project: &mut Project) {
        let removed: HashSet<&str> = self.remove.iter().map(|e| e.id.as_str()).collect();
        project.content.retain(|e| !removed.contains(e.id.as_str()));
        for entry in &mut project.content {
            entry.requires.retain(|r| !removed.contains(r.as_str()));
        }
        for (_, new) in &self.update {
            if let Some(slot) = project.content.iter_mut().find(|e| e.id == new.id) {
                *slot = new.clone();
            }
        }
        project.content.extend(self.add.iter().cloned());
    }
}

fn kind_dir(kind: Kind) -> &'static str {
    match kind {
        Kind::Mod => "mods",
        Kind::ResourcePack => "resourcepacks",
        Kind::ShaderPack => "shaderpacks",
        Kind::DataPack => "datapacks",
        Kind::File => "files",
    }
}

/// Entries of reason `dependency` that no explicit entry needs any more.
pub fn orphans(content: &[Entry]) -> Vec<String> {
    let by_id: HashMap<&str, &Entry> = content.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut needed: HashSet<&str> = HashSet::new();
    let mut stack: Vec<&str> = content
        .iter()
        .filter(|e| e.reason == Reason::Explicit)
        .map(|e| e.id.as_str())
        .collect();
    while let Some(id) = stack.pop() {
        if needed.insert(id)
            && let Some(entry) = by_id.get(id)
        {
            stack.extend(entry.requires.iter().map(String::as_str));
        }
    }
    content
        .iter()
        .filter(|e| !needed.contains(e.id.as_str()))
        .map(|e| e.id.clone())
        .collect()
}

/// Plans `add`, `update` and `remove` against one content source.
pub struct Planner<'a, S, J> {
    source: &'a S,
    jars: &'a J,
    pack: &'a Project,
    plan: Plan,
    metas: HashMap<String, Option<JarMeta>>,
    tried_mod_ids: HashSet<String>,
}

struct Pending {
    parent: String,
    version: Version,
    side: Side,
}

impl<'a, S: Source, J: JarFetcher> Planner<'a, S, J> {
    pub fn new(source: &'a S, jars: &'a J, pack: &'a Project) -> Self {
        Self {
            source,
            jars,
            pack,
            plan: Plan::default(),
            metas: HashMap::new(),
            tried_mod_ids: HashSet::new(),
        }
    }

    fn target(&self, kind: Kind) -> Target {
        Target {
            minecraft: self.pack.minecraft.clone(),
            loader: self.pack.loader.kind,
            kind,
        }
    }

    fn env(&self) -> Env {
        Env {
            minecraft: ModVersion::parse(&self.pack.minecraft),
            loader: self.pack.loader.kind,
            loader_version: ModVersion::parse(&self.pack.loader.version),
            java: self.pack.java.major,
        }
    }

    /// The pack's content as it would be after the plan so far.
    fn content(&self) -> Vec<Entry> {
        let mut project = self.pack.clone();
        self.plan.apply(&mut project);
        project.content
    }

    fn entry_for_project(&self, project_id: &str) -> Option<String> {
        self.content()
            .into_iter()
            .find(|e| matches!(&e.source, EntrySource::Modrinth { project, .. } if project == project_id))
            .map(|e| e.id)
    }

    fn planned_mut(&mut self, id: &str) -> Option<&mut Entry> {
        self.plan
            .add
            .iter_mut()
            .chain(self.plan.update.iter_mut().map(|(_, new)| new))
            .find(|e| e.id == id)
    }

    fn fresh_id(&self, slug: &str) -> String {
        let taken: HashSet<String> = self.content().into_iter().map(|e| e.id).collect();
        let base = slug.to_ascii_lowercase();
        (1..)
            .map(|n| {
                if n == 1 {
                    base.clone()
                } else {
                    format!("{base}-{n}")
                }
            })
            .find(|id| !taken.contains(id))
            .expect("some suffix is free")
    }

    fn compatible(&self, version: &Version, kind: Kind) -> bool {
        let on_mc = version.game_versions.contains(&self.pack.minecraft);
        let loader = self.pack.loader.kind;
        let on_loader = kind != Kind::Mod
            || version.loaders.iter().any(|l| {
                l == loader_name(loader) || (loader == LoaderKind::Quilt && l == "fabric")
            });
        on_mc && on_loader
    }

    /// The newest version of any channel; the author decides what is stable enough.
    fn choose(&self, versions: &[Version]) -> Option<Version> {
        versions.first().cloned()
    }

    async fn compatible_versions(
        &self,
        info: &ProjectInfo,
        kind: Kind,
    ) -> Result<Vec<Version>, ResolveError> {
        Ok(self.source.resolve(&info.id, &self.target(kind)).await?)
    }

    fn no_version(&self, info: &ProjectInfo) -> ResolveError {
        ResolveError::NoCompatibleVersion {
            project: info.slug.clone(),
            minecraft: self.pack.minecraft.clone(),
            loader: loader_name(self.pack.loader.kind).into(),
        }
    }

    fn build_entry(
        &self,
        id: String,
        info: &ProjectInfo,
        version: &Version,
        kind: Kind,
    ) -> Result<Entry, ResolveError> {
        let file = version
            .primary_file()
            .ok_or_else(|| ResolveError::NoFile(version.id.clone()))?;
        let path =
            PackPath::new(format!("{}/{}", kind_dir(kind), file.filename)).map_err(|source| {
                ResolveError::BadPath {
                    version: version.id.clone(),
                    source,
                }
            })?;
        Ok(Entry {
            id,
            kind,
            name: info.title.clone(),
            source: EntrySource::Modrinth {
                project: info.id.clone(),
                version: version.id.clone(),
            },
            file: EntryFile {
                path,
                size: file.size,
                hashes: file.hashes.clone(),
                url: file.url.clone(),
            },
            side: info.side(),
            group: None,
            update: UpdatePolicy::Follow,
            reason: Reason::Explicit,
            requires: Vec::new(),
        })
    }

    /// Plans adding a project and everything it requires.
    pub async fn add(mut self, project: &str, options: &AddOptions) -> Result<Plan, ResolveError> {
        let info = self.source.project(project).await?;
        if let Some(id) = self.entry_for_project(&info.id) {
            return Err(ResolveError::AlreadyPresent {
                project: info.slug,
                id,
            });
        }
        let kind = info
            .kind
            .ok_or_else(|| ResolveError::Modpack(info.slug.clone()))?;
        let versions = self.compatible_versions(&info, kind).await?;
        let version = match &options.version {
            Some(wanted) => versions
                .iter()
                .find(|v| v.id == *wanted || v.number == *wanted)
                .cloned()
                .ok_or_else(|| ResolveError::NoSuchVersion {
                    project: info.slug.clone(),
                    version: wanted.clone(),
                    minecraft: self.pack.minecraft.clone(),
                    loader: loader_name(self.pack.loader.kind).into(),
                })?,
            None => self
                .choose(&versions)
                .ok_or_else(|| self.no_version(&info))?,
        };
        let mut entry = self.build_entry(self.fresh_id(&info.slug), &info, &version, kind)?;
        if let Some(side) = options.side {
            entry.side = side;
        }
        entry.group = options.group.clone();
        if options.pin {
            entry.update = UpdatePolicy::Pinned;
        }
        let pending = Pending {
            parent: entry.id.clone(),
            side: entry.side,
            version,
        };
        self.plan.add.push(entry);
        self.resolve_dependencies(vec![pending]).await?;
        self.metadata_rounds().await?;
        self.link_requires().await;
        Ok(self.finalize().await)
    }

    /// Plans updating `ids` (all `follow` entries when empty) to their newest versions.
    pub async fn update(mut self, ids: &[String]) -> Result<Plan, ResolveError> {
        for id in ids {
            if self.pack.entry(id).is_none() {
                return Err(ResolveError::UnknownEntry(id.clone()));
            }
        }
        let mut pending = Vec::new();
        for entry in &self.pack.content {
            if !ids.is_empty() && !ids.contains(&entry.id) {
                continue;
            }
            let EntrySource::Modrinth { project, version } = &entry.source else {
                continue;
            };
            if entry.update == UpdatePolicy::Pinned {
                if !ids.is_empty() {
                    self.plan.notes.push(format!("`{}` is pinned", entry.id));
                }
                continue;
            }
            let current = self.source.version(version).await?;
            let info = self.source.project(project).await?;
            let newer: Vec<Version> = self
                .compatible_versions(&info, entry.kind)
                .await?
                .into_iter()
                .filter(|v| v.published > current.published)
                .collect();
            let Some(next) = self.choose(&newer) else {
                continue;
            };
            let mut new = self.build_entry(entry.id.clone(), &info, &next, entry.kind)?;
            new.side = entry.side;
            new.group = entry.group.clone();
            new.update = entry.update;
            new.reason = entry.reason;
            pending.push(Pending {
                parent: new.id.clone(),
                side: new.side,
                version: next,
            });
            self.plan.update.push((entry.clone(), new));
        }
        self.resolve_dependencies(pending).await?;
        self.metadata_rounds().await?;
        self.link_requires().await;
        self.plan_orphans();
        Ok(self.finalize().await)
    }

    /// Plans removing `id` and, unless `keep_deps`, dependencies left unused.
    pub fn remove(mut self, id: &str, keep_deps: bool) -> Result<Plan, ResolveError> {
        let entry = self
            .pack
            .entry(id)
            .ok_or_else(|| ResolveError::UnknownEntry(id.to_owned()))?;
        self.plan.remove.push(entry.clone());
        for other in &self.pack.content {
            if other.id != id && other.requires.iter().any(|r| r == id) {
                self.plan
                    .notes
                    .push(format!("`{}` requires `{id}`", other.id));
            }
        }
        if !keep_deps {
            self.plan_orphans();
        }
        Ok(self.plan)
    }

    async fn resolve_dependencies(&mut self, mut queue: Vec<Pending>) -> Result<(), ResolveError> {
        while let Some(Pending {
            parent,
            version,
            side,
        }) = queue.pop()
        {
            let deps = self.source.dependencies(&version).await?;
            for dep in deps {
                let project = match (&dep.project, &dep.version) {
                    (Some(project), _) => project.clone(),
                    (None, Some(version)) => self.source.version(version).await?.project,
                    (None, None) => continue,
                };
                match dep.kind {
                    DependencyKind::Required => {}
                    DependencyKind::Incompatible => {
                        if let Some(id) = self.entry_for_project(&project) {
                            self.plan
                                .notes
                                .push(format!("`{parent}` is incompatible with `{id}`"));
                        }
                        continue;
                    }
                    _ => continue,
                }
                if let Some(id) = self.entry_for_project(&project) {
                    self.require(&parent, id);
                    continue;
                }
                let info = self.source.project(&project).await?;
                let kind = info.kind.unwrap_or(Kind::Mod);
                let pinned = match &dep.version {
                    Some(v) => {
                        Some(self.source.version(v).await?).filter(|v| self.compatible(v, kind))
                    }
                    None => None,
                };
                let version = match pinned {
                    Some(v) => v,
                    None => {
                        let versions = self.compatible_versions(&info, kind).await?;
                        match self.pick_for(&parent, &info, &versions, kind).await? {
                            Some(v) => v,
                            None => self
                                .choose(&versions)
                                .ok_or_else(|| self.no_version(&info))?,
                        }
                    }
                };
                let id = self.add_dependency(&info, &version, kind, side)?;
                self.require(&parent, id.clone());
                queue.push(Pending {
                    parent: id,
                    side,
                    version,
                });
            }
        }
        Ok(())
    }

    fn add_dependency(
        &mut self,
        info: &ProjectInfo,
        version: &Version,
        kind: Kind,
        parent_side: Side,
    ) -> Result<String, ResolveError> {
        let mut entry = self.build_entry(self.fresh_id(&info.slug), info, version, kind)?;
        if entry.side == Side::Both {
            entry.side = parent_side;
        }
        entry.reason = Reason::Dependency;
        let id = entry.id.clone();
        self.plan.add.push(entry);
        Ok(id)
    }

    fn require(&mut self, parent: &str, id: String) {
        if let Some(entry) = self.planned_mut(parent)
            && !entry.requires.contains(&id)
        {
            entry.requires.push(id);
        }
    }

    async fn meta(&mut self, entry: &Entry) -> Option<JarMeta> {
        if entry.kind != Kind::Mod {
            return None;
        }
        let key = format!("{:?}", entry.file.hashes.strongest());
        if let Some(meta) = self.metas.get(&key) {
            return meta.clone();
        }
        let meta = match self.jars.jar(&entry.file).await {
            Ok(bytes) => match JarMeta::read(&bytes) {
                Ok(meta) => Some(meta),
                Err(e) => {
                    self.plan
                        .notes
                        .push(format!("cannot inspect `{}`: {e}", entry.id));
                    None
                }
            },
            Err(e) => {
                self.plan
                    .notes
                    .push(format!("cannot download `{}`: {e}", entry.id));
                None
            }
        };
        self.metas.insert(key, meta.clone());
        meta
    }

    async fn problems(&mut self) -> Vec<Problem> {
        let content = self.content();
        let mut metas = Vec::new();
        for entry in &content {
            if let Some(meta) = self.meta(entry).await {
                metas.push((entry, meta));
            }
        }
        let installed: Vec<Installed> = metas
            .iter()
            .map(|(e, meta)| Installed {
                entry: &e.id,
                side: e.side,
                meta,
            })
            .collect();
        check(&self.env(), &installed)
    }

    fn is_planned(&self, id: &str) -> bool {
        self.plan.add.iter().any(|e| e.id == id) || self.plan.update.iter().any(|(_, n)| n.id == id)
    }

    /// Fills dependencies only jar metadata declares, by looking up a project with the mod id as slug.
    async fn metadata_rounds(&mut self) -> Result<(), ResolveError> {
        for _ in 0..METADATA_ROUNDS {
            let missing: BTreeSet<(String, String)> = self
                .problems()
                .await
                .iter()
                .filter_map(|p| match p {
                    Problem::Missing { entry, dep, .. } if self.is_planned(entry) => {
                        Some((dep.clone(), entry.clone()))
                    }
                    _ => None,
                })
                .collect();
            let mut added = false;
            for (mod_id, parent) in missing {
                if !self.tried_mod_ids.insert(mod_id.clone()) {
                    continue;
                }
                if let Some(id) = self.resolve_mod_id(&mod_id, &parent).await? {
                    self.require(&parent, id);
                    added = true;
                }
            }
            if !added {
                break;
            }
        }
        Ok(())
    }

    /// Adds to `requires` every entry providing a mod id a planned jar requires.
    async fn link_requires(&mut self) {
        let loader = self.pack.loader.kind;
        // mod id → (entry, provided by the jar itself rather than a nested library)
        let mut providers: HashMap<String, Vec<(String, bool)>> = HashMap::new();
        let mut planned_deps: Vec<(String, Vec<String>)> = Vec::new();
        for entry in self.content() {
            let Some(meta) = self.meta(&entry).await else {
                continue;
            };
            let top = meta.mods.iter().filter(|m| m.platform.runs_on(loader));
            let nested = meta.nested.iter().flat_map(|n| n.mods_for(loader));
            for (m, top_level) in top.map(|m| (m, true)).chain(nested.map(|m| (m, false))) {
                for id in std::iter::once(&m.id).chain(&m.provides) {
                    providers
                        .entry(id.clone())
                        .or_default()
                        .push((entry.id.clone(), top_level));
                }
            }
            if self.is_planned(&entry.id) {
                let deps = meta
                    .mods
                    .iter()
                    .filter(|m| m.platform.runs_on(loader))
                    .flat_map(|m| &m.deps)
                    .filter(|d| d.kind == DepKind::Required)
                    .map(|d| d.id.clone())
                    .collect();
                planned_deps.push((entry.id.clone(), deps));
            }
        }
        for (id, deps) in planned_deps {
            let requires = self
                .planned_mut(&id)
                .map(|e| e.requires.clone())
                .unwrap_or_default();
            for dep in deps {
                let Some(candidates) = providers.get(&dep) else {
                    continue;
                };
                if candidates.iter().any(|(e, _)| *e == id) {
                    continue;
                }
                let chosen = candidates
                    .iter()
                    .find(|(e, _)| requires.contains(e))
                    .or_else(|| candidates.iter().find(|(_, top)| *top))
                    .or_else(|| candidates.first());
                if let Some((provider, _)) = chosen {
                    self.require(&id, provider.clone());
                }
            }
        }
    }

    /// What `parent`'s jar requires of other mods (version ranges included).
    async fn parent_requirements(&mut self, parent: &str) -> Vec<ModDep> {
        let loader = self.pack.loader.kind;
        let Some(entry) = self.content().into_iter().find(|e| e.id == parent) else {
            return vec![];
        };
        let Some(meta) = self.meta(&entry).await else {
            return vec![];
        };
        meta.mods
            .iter()
            .filter(|m| m.platform.runs_on(loader))
            .flat_map(|m| m.deps.iter().cloned())
            .filter(|d| matches!(d.kind, DepKind::Required | DepKind::Optional))
            .collect()
    }

    /// Newest version whose jar satisfies `parent`'s version ranges.
    async fn pick_for(
        &mut self,
        parent: &str,
        info: &ProjectInfo,
        versions: &[Version],
        kind: Kind,
    ) -> Result<Option<Version>, ResolveError> {
        let reqs = self.parent_requirements(parent).await;
        if kind != Kind::Mod || reqs.iter().all(|d| d.req.is_any()) {
            return Ok(self.choose(versions));
        }
        // Version numbers often embed the mod version; try plausible ones first.
        let slug_ids = [info.slug.clone(), info.slug.replace('-', "_")];
        let hinted = |v: &Version| {
            v.number.split(['-', '+', '_']).any(|piece| {
                let piece = ModVersion::parse(piece.trim_start_matches("mc"));
                reqs.iter()
                    .filter(|d| slug_ids.contains(&d.id))
                    .any(|d| d.req.matches(&piece))
            })
        };
        let ordered: Vec<&Version> = versions.iter().collect();
        let (mut candidates, rest): (Vec<&Version>, Vec<&Version>) =
            ordered.into_iter().partition(|v| hinted(v));
        candidates.extend(rest);

        let loader = self.pack.loader.kind;
        for version in candidates.into_iter().take(MAX_CANDIDATES) {
            let candidate = self.build_entry(info.slug.clone(), info, version, kind)?;
            let Some(meta) = self.meta(&candidate).await else {
                continue;
            };
            let accepted = meta.mods_for(loader).iter().all(|m| {
                reqs.iter()
                    .filter(|d| d.id == m.id || m.provides.contains(&d.id))
                    .all(|d| d.req.matches(&m.version))
            });
            if accepted {
                return Ok(Some(version.clone()));
            }
        }
        Ok(None)
    }

    async fn finalize(mut self) -> Plan {
        let problems = self.problems().await;
        self.plan.problems = problems
            .into_iter()
            .filter(|p| self.is_planned(p.entry()))
            .collect();
        self.plan
    }

    /// Removes entries this plan leaves unneeded; pre-existing orphans stay.
    fn plan_orphans(&mut self) {
        let before = orphans(&self.pack.content);
        let content = self.content();
        let after = orphans(&content);
        self.plan.remove.extend(
            content
                .into_iter()
                .filter(|e| after.contains(&e.id) && !before.contains(&e.id)),
        );
        let removed: HashSet<String> = self.plan.remove.iter().map(|e| e.id.clone()).collect();
        self.plan
            .update
            .retain(|(_, new)| !removed.contains(&new.id));
        self.plan.add.retain(|e| !removed.contains(&e.id));
    }

    async fn resolve_mod_id(
        &mut self,
        mod_id: &str,
        parent: &str,
    ) -> Result<Option<String>, ResolveError> {
        let mut slugs = vec![mod_id.to_owned()];
        if mod_id.contains('_') {
            slugs.push(mod_id.replace('_', "-"));
        }
        for slug in slugs {
            let info = match self.source.project(&slug).await {
                Ok(info) => info,
                Err(riven_sources::Error::NotFound(_)) => continue,
                Err(e) => return Err(e.into()),
            };
            if info.kind != Some(Kind::Mod) {
                continue;
            }
            if let Some(id) = self.entry_for_project(&info.id) {
                return Ok(Some(id));
            }
            let versions = self.compatible_versions(&info, Kind::Mod).await?;
            let picked = self.pick_for(parent, &info, &versions, Kind::Mod).await?;
            let Some(version) = picked.or_else(|| self.choose(&versions)) else {
                continue;
            };
            let side = self
                .planned_mut(parent)
                .map(|e| e.side)
                .unwrap_or(Side::Both);
            let candidate = self.build_entry(String::new(), &info, &version, Kind::Mod)?;
            let provides = self.meta(&candidate).await.is_some_and(|meta| {
                meta.mods_for(self.pack.loader.kind)
                    .iter()
                    .any(|m| m.id == mod_id || m.provides.iter().any(|p| p == mod_id))
            });
            if !provides {
                continue;
            }
            let id = self.add_dependency(&info, &version, Kind::Mod, side)?;
            self.resolve_dependencies(vec![Pending {
                parent: id.clone(),
                side,
                version,
            }])
            .await?;
            return Ok(Some(id));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use riven_format::{FileRules, Hashes, Java, Loader};
    use riven_sources::{Channel, Dependency, Hit, Support, VersionFile};

    use super::*;
    use crate::meta::tests::{jar, neoforge_jar};

    #[derive(Default)]
    struct Fake {
        projects: Vec<ProjectInfo>,
        versions: Vec<Version>,
        jars: HashMap<String, Vec<u8>>,
    }

    impl Fake {
        fn add_project(&mut self, id: &str, slug: &str, client: Support, server: Support) {
            self.projects.push(ProjectInfo {
                id: id.into(),
                slug: slug.into(),
                title: slug.into(),
                description: String::new(),
                kind: Some(Kind::Mod),
                client,
                server,
                owner: "team".into(),
            });
        }

        #[allow(clippy::too_many_arguments)]
        fn add_version(
            &mut self,
            id: &str,
            project: &str,
            published: &str,
            channel: Channel,
            deps: Vec<Dependency>,
            mod_id: &str,
            mod_deps: &str,
        ) {
            let jar = neoforge_jar(mod_id, "1.0", &mod_deps.replace("{ID}", mod_id));
            self.add_version_jar(id, project, published, channel, deps, jar);
        }

        fn add_version_jar(
            &mut self,
            id: &str,
            project: &str,
            published: &str,
            channel: Channel,
            deps: Vec<Dependency>,
            jar: Vec<u8>,
        ) {
            let url = format!("https://cdn.test/{id}.jar");
            self.jars.insert(url.clone(), jar);
            self.versions.push(Version {
                id: id.into(),
                project: project.into(),
                name: id.into(),
                number: id.into(),
                channel,
                game_versions: vec!["1.21.1".into()],
                loaders: vec!["neoforge".into()],
                published: published.into(),
                files: vec![VersionFile {
                    filename: format!("{id}.jar"),
                    url: Some(url),
                    size: 1,
                    hashes: Hashes {
                        sha512: Some(format!("{id:0>128}")),
                        ..Hashes::default()
                    },
                    primary: true,
                }],
                dependencies: deps,
            });
        }
    }

    fn not_found(what: &str) -> riven_sources::Error {
        riven_sources::Error::NotFound(what.into())
    }

    impl Source for Fake {
        fn kind(&self) -> riven_format::SourceKind {
            riven_format::SourceKind::Modrinth
        }

        async fn search(&self, _: &str, _: &Target, _: u32) -> riven_sources::Result<Vec<Hit>> {
            Ok(vec![])
        }

        async fn project(&self, id: &str) -> riven_sources::Result<ProjectInfo> {
            self.projects
                .iter()
                .find(|p| p.id == id || p.slug == id)
                .cloned()
                .ok_or_else(|| not_found(id))
        }

        async fn resolve(&self, project: &str, t: &Target) -> riven_sources::Result<Vec<Version>> {
            let mut out: Vec<Version> = self
                .versions
                .iter()
                .filter(|v| v.project == project && v.game_versions.contains(&t.minecraft))
                .cloned()
                .collect();
            out.sort_by(|a, b| b.published.cmp(&a.published));
            Ok(out)
        }

        async fn version(&self, id: &str) -> riven_sources::Result<Version> {
            self.versions
                .iter()
                .find(|v| v.id == id)
                .cloned()
                .ok_or_else(|| not_found(id))
        }
    }

    impl JarFetcher for Fake {
        async fn jar(&self, file: &EntryFile) -> Result<Vec<u8>, String> {
            let url = file.url.as_ref().ok_or("no url")?;
            self.jars
                .get(url)
                .cloned()
                .ok_or_else(|| format!("{url} missing"))
        }
    }

    fn pack() -> Project {
        Project {
            name: "Test".into(),
            id: "test".into(),
            version: "1.0.0".into(),
            authors: vec![],
            description: None,
            icon: None,
            minecraft: "1.21.1".into(),
            loader: Loader {
                kind: LoaderKind::NeoForge,
                version: "21.1.219".into(),
            },
            java: Java {
                major: 21,
                memory: None,
                jvm_args: vec![],
            },
            groups: vec![],
            files: FileRules::default(),
            content: vec![],
        }
    }

    fn requires(dep: &str) -> String {
        format!(
            "[[dependencies.{{ID}}]]\nmodId = \"{dep}\"\ntype = \"required\"\nversionRange = \"[1.0,)\"\n"
        )
    }

    fn required(project: &str, version: Option<&str>) -> Dependency {
        Dependency {
            project: Some(project.into()),
            version: version.map(Into::into),
            kind: DependencyKind::Required,
        }
    }

    fn ids(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.id.as_str()).collect()
    }

    const DAY1: &str = "2026-01-01T00:00:00Z";
    const DAY2: &str = "2026-02-01T00:00:00Z";
    const DAY3: &str = "2026-03-01T00:00:00Z";

    fn create_world() -> Fake {
        let both = (Support::Required, Support::Required);
        let mut fake = Fake::default();
        fake.add_project("C", "create", both.0, both.1);
        fake.add_project("P", "ponder", both.0, both.1);
        fake.add_project("R", "registrate", both.0, both.1);
        fake.add_version(
            "c1",
            "C",
            DAY1,
            Channel::Release,
            vec![],
            "create",
            &requires("ponder"),
        );
        fake.add_version(
            "p1",
            "P",
            DAY1,
            Channel::Release,
            vec![required("R", Some("r1"))],
            "ponder",
            "",
        );
        fake.add_version("r1", "R", DAY1, Channel::Release, vec![], "registrate", "");
        fake.add_version("r2", "R", DAY2, Channel::Release, vec![], "registrate", "");
        fake
    }

    #[tokio::test]
    async fn add_follows_api_and_jar_metadata_dependencies() {
        let fake = create_world();
        let pack = pack();
        let plan = Planner::new(&fake, &fake, &pack)
            .add("create", &AddOptions::default())
            .await
            .unwrap();

        assert_eq!(ids(&plan.add), ["create", "ponder", "registrate"]);
        assert!(plan.problems.is_empty(), "{:?}", plan.problems);
        let by_id = |id: &str| plan.add.iter().find(|e| e.id == id).unwrap();
        assert_eq!(by_id("create").requires, ["ponder"]);
        assert_eq!(by_id("create").reason, Reason::Explicit);
        assert_eq!(by_id("ponder").reason, Reason::Dependency);
        assert_eq!(by_id("ponder").requires, ["registrate"]);
        assert!(matches!(&by_id("registrate").source,
            EntrySource::Modrinth { version, .. } if version == "r1"));

        let mut pack = pack;
        plan.apply(&mut pack);
        let again = Planner::new(&fake, &fake, &pack)
            .add("create", &AddOptions::default())
            .await;
        assert!(matches!(again, Err(ResolveError::AlreadyPresent { .. })));
    }

    #[tokio::test]
    async fn update_takes_newest_of_any_channel_and_drops_new_orphans() {
        let mut fake = create_world();
        let mut pack = pack();
        Planner::new(&fake, &fake, &pack)
            .add("create", &AddOptions::default())
            .await
            .unwrap()
            .apply(&mut pack);
        fake.add_version("c2", "C", DAY2, Channel::Release, vec![], "create", "");
        fake.add_version("c3", "C", DAY3, Channel::Beta, vec![], "create", "");
        let plan = Planner::new(&fake, &fake, &pack).update(&[]).await.unwrap();

        let updated: Vec<_> = plan.update.iter().map(|(_, n)| &n.source).collect();
        assert!(matches!(updated[..],
            [EntrySource::Modrinth { version, .. }] if version == "c3"));
        assert_eq!(ids(&plan.remove), ["ponder", "registrate"]);

        let mut pinned = pack.clone();
        pinned
            .content
            .iter_mut()
            .for_each(|e| e.update = UpdatePolicy::Pinned);
        let plan = Planner::new(&fake, &fake, &pinned)
            .update(&["create".into()])
            .await
            .unwrap();
        assert!(plan.is_empty());
        assert_eq!(plan.notes, ["`create` is pinned"]);
    }

    #[tokio::test]
    async fn remove_drops_unused_dependencies_unless_kept() {
        let fake = create_world();
        let mut pack = pack();
        Planner::new(&fake, &fake, &pack)
            .add("create", &AddOptions::default())
            .await
            .unwrap()
            .apply(&mut pack);

        let plan = Planner::new(&fake, &fake, &pack)
            .remove("create", false)
            .unwrap();
        assert_eq!(ids(&plan.remove), ["create", "ponder", "registrate"]);

        let plan = Planner::new(&fake, &fake, &pack)
            .remove("create", true)
            .unwrap();
        assert_eq!(ids(&plan.remove), ["create"]);

        let plan = Planner::new(&fake, &fake, &pack)
            .remove("ponder", false)
            .unwrap();
        assert_eq!(ids(&plan.remove), ["ponder", "registrate"]);
        assert_eq!(plan.notes, ["`create` requires `ponder`"]);
        plan.apply(&mut pack);
        assert!(pack.validate().is_empty());
        assert!(pack.entry("create").unwrap().requires.is_empty());
    }

    #[tokio::test]
    async fn dependency_version_honours_parent_range() {
        let both = (Support::Required, Support::Required);
        let mut fake = Fake::default();
        fake.add_project("I", "iris", both.0, both.1);
        fake.add_project("S", "sodium", both.0, both.1);
        let iris = neoforge_jar(
            "iris",
            "1.8.8",
            "[[dependencies.iris]]\nmodId = \"sodium\"\ntype = \"required\"\nversionRange = \"[0.6,0.7)\"\n",
        );
        fake.add_version_jar(
            "i1",
            "I",
            DAY1,
            Channel::Release,
            vec![required("S", None)],
            iris,
        );
        let sodium = |v: &str| neoforge_jar("sodium", v, "");
        fake.add_version_jar("s6", "S", DAY1, Channel::Release, vec![], sodium("0.6.13"));
        fake.add_version_jar("s8", "S", DAY2, Channel::Release, vec![], sodium("0.8.13"));

        let pack = pack();
        let plan = Planner::new(&fake, &fake, &pack)
            .add("iris", &AddOptions::default())
            .await
            .unwrap();
        let sodium = plan.add.iter().find(|e| e.id == "sodium").unwrap();
        assert!(matches!(&sodium.source, EntrySource::Modrinth { version, .. } if version == "s6"));
        assert!(plan.problems.is_empty(), "{:?}", plan.problems);
    }

    #[tokio::test]
    async fn requires_link_prefers_declared_over_nested_copies() {
        let both = (Support::Required, Support::Required);
        let mut fake = Fake::default();
        fake.add_project("I", "iris", both.0, both.1);
        fake.add_project("A", "fabric-api", both.0, both.1);
        fake.add_project("M", "modmenu", both.0, both.1);
        let bundling = |id: &str| {
            let module = neoforge_jar("key-binding-api", "1.0", "");
            jar(&[
                ("META-INF/neoforge.mods.toml", format!("modLoader=\"javafml\"\nloaderVersion=\"[1,)\"\n[[mods]]\nmodId=\"{id}\"\nversion=\"1\"\n").as_bytes()),
                ("META-INF/jarjar/metadata.json", br#"{"jars":[{"path":"META-INF/jarjar/m.jar"}]}"#),
                ("META-INF/jarjar/m.jar", &module),
            ])
        };
        fake.add_version_jar("i1", "I", DAY1, Channel::Release, vec![], bundling("iris"));
        fake.add_version_jar(
            "a1",
            "A",
            DAY1,
            Channel::Release,
            vec![],
            bundling("fabric_api"),
        );
        fake.add_version(
            "m1",
            "M",
            DAY1,
            Channel::Release,
            vec![required("A", None)],
            "modmenu",
            &requires("key-binding-api"),
        );

        let mut pack = pack();
        Planner::new(&fake, &fake, &pack)
            .add("iris", &AddOptions::default())
            .await
            .unwrap()
            .apply(&mut pack);
        let plan = Planner::new(&fake, &fake, &pack)
            .add("modmenu", &AddOptions::default())
            .await
            .unwrap();
        let modmenu = plan.add.iter().find(|e| e.id == "modmenu").unwrap();
        assert_eq!(modmenu.requires, ["fabric-api"]);
    }
}
