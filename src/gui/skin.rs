use std::path::{Path, PathBuf};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, img, px,
    relative,
};
use moonglass::theme::ActiveTheme as _;
use moonglass::toast::ToastKind;
use moonglass::{Button, ButtonSize, h_flex, v_flex};
use riven_format::{Account, AccountKind};
use riven_play::skins::SkinModel;
use rust_i18n::t;
use sha2::Digest as _;

use super::dialogs::{dialog_shell, leave, open};
use super::play::PlayState;
use super::runtime;
use super::state::AppState;

/// A skin PNG in the library, with its pictures rendered once.
#[derive(Clone)]
struct Item {
    hash: String,
    png: PathBuf,
    head: PathBuf,
}

/// Where a skin goes: Mojang for a Microsoft account, the nick on BX ID for an offline one.
#[derive(Clone, Copy, PartialEq)]
enum Target {
    Microsoft,
    Nick,
}

/// Changes the skin an account wears, from a library of skins kept on this computer.
pub struct SkinDialog {
    account: Account,
    target: Target,
    /// What the account wears now, once read.
    current: Option<Item>,
    current_slim: bool,
    loading: bool,
    picked: Option<Item>,
    slim: bool,
    library: Vec<Item>,
    /// Skins this BX ID uploaded, newest first, with whether each was worn with slim arms.
    cloud: Vec<(Item, bool)>,
    busy: bool,
    error: Option<SharedString>,
}

pub fn open_skin(account: Account, cx: &mut App) {
    let view = cx.new(|cx| SkinDialog::new(account, cx));
    open(view.into(), 560., cx);
}

/// `<data>/riven/skins/<sha256>.png`: every skin the player picked or wore, so they can go back to it.
fn library_dir() -> Option<PathBuf> {
    riven_sync::data_dir().map(|d| d.join("skins"))
}

fn pictures_dir() -> Option<PathBuf> {
    super::mods::cache_dir().map(|d| d.join("skins"))
}

/// Checks that `png` is a skin and keeps it in the library.
fn keep(png: &[u8]) -> Result<Item, String> {
    let image = image::load_from_memory(png).map_err(|_| t!("skin.not_png").to_string())?;
    if image.width() != 64 || !(image.height() == 64 || image.height() == 32) {
        return Err(t!("skin.bad_size", w = image.width(), h = image.height()).to_string());
    }
    let hash = hex::encode(sha2::Sha256::digest(png));
    let dir = library_dir().ok_or_else(|| t!("skin.no_dir").to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{hash}.png"));
    if !path.is_file() {
        std::fs::write(&path, png).map_err(|e| e.to_string())?;
    }
    item(&path, &hash).ok_or_else(|| t!("skin.not_png").to_string())
}

fn item(png: &Path, hash: &str) -> Option<Item> {
    let dir = pictures_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let head = dir.join(format!("library-{}.png", &hash[..16]));
    if !head.is_file() {
        std::fs::write(&head, super::mods::skin_head(&std::fs::read(png).ok()?)?).ok()?;
    }
    Some(Item {
        hash: hash.to_owned(),
        png: png.to_owned(),
        head,
    })
}

/// The library, newest first.
fn library() -> Vec<Item> {
    let Some(dir) = library_dir() else {
        return Vec::new();
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "png"))
        .map(|e| {
            let when = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (when, e.path())
        })
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files
        .into_iter()
        .filter_map(|(_, path)| {
            let hash = path.file_stem()?.to_str()?.to_owned();
            item(&path, &hash)
        })
        .collect()
}

/// The skin from the front and from the back, side by side, for `slim` or classic arms.
fn preview(item: &Item, slim: bool) -> Option<PathBuf> {
    let dir = pictures_dir()?;
    let path = dir.join(format!(
        "preview-{}-{}.png",
        &item.hash[..16],
        if slim { "slim" } else { "classic" }
    ));
    if !path.is_file() {
        let png = render(&std::fs::read(&item.png).ok()?, slim)?;
        std::fs::write(&path, png).ok()?;
    }
    Some(path)
}

/// Flat front and back views of a skin, scaled up without blur, as PNG bytes.
pub fn render(skin: &[u8], slim: bool) -> Option<Vec<u8>> {
    use image::imageops::{self, FilterType};
    use image::{Rgba, RgbaImage};
    let skin = image::load_from_memory(skin).ok()?.to_rgba8();
    if skin.width() != 64 {
        return None;
    }
    // Skins from before 1.8 are 64×32: no second layer but the hat, and the left limbs mirror the right ones.
    let legacy = skin.height() == 32;
    let arm = if slim { 3 } else { 4 };
    let part = |x: u32, y: u32, w: u32, h: u32| imageops::crop_imm(&skin, x, y, w, h).to_image();
    let mirrored = |x, y, w, h| imageops::flip_horizontal(&part(x, y, w, h));
    // Old skins fill the hat area opaque; the game then treats the layer as empty.
    let hat_used = |x, y| part(x, y, 8, 8).pixels().any(|p| p[3] < 255);

    let mut out = RgbaImage::from_pixel(16 * 2 + 4, 32, Rgba([0, 0, 0, 0]));
    let mut put = |image: &RgbaImage, x: i64, y: i64| imageops::overlay(&mut out, image, x, y);

    // Front: the player's right side is on the viewer's left.
    put(&part(8, 8, 8, 8), 4, 0);
    if hat_used(40, 8) {
        put(&part(40, 8, 8, 8), 4, 0);
    }
    put(&part(20, 20, 8, 12), 4, 8);
    put(&part(44, 20, arm, 12), i64::from(4 - arm), 8);
    put(&part(4, 20, 4, 12), 4, 20);
    if legacy {
        put(&mirrored(44, 20, arm, 12), 12, 8);
        put(&mirrored(4, 20, 4, 12), 8, 20);
    } else {
        put(&part(36, 52, arm, 12), 12, 8);
        put(&part(20, 52, 4, 12), 8, 20);
        put(&part(20, 36, 8, 12), 4, 8);
        put(&part(44, 36, arm, 12), i64::from(4 - arm), 8);
        put(&part(52, 52, arm, 12), 12, 8);
        put(&part(4, 36, 4, 12), 4, 20);
        put(&part(4, 52, 4, 12), 8, 20);
    }

    // Back, 20 pixels to the right: the player's left side is now on the viewer's left.
    let b = 20;
    put(&part(24, 8, 8, 8), b + 4, 0);
    if hat_used(56, 8) {
        put(&part(56, 8, 8, 8), b + 4, 0);
    }
    put(&part(32, 20, 8, 12), b + 4, 8);
    put(&part(44 + 4 + arm, 20, arm, 12), b + 12, 8);
    put(&part(12, 20, 4, 12), b + 8, 20);
    if legacy {
        put(
            &mirrored(44 + 4 + arm, 20, arm, 12),
            b + i64::from(4 - arm),
            8,
        );
        put(&mirrored(12, 20, 4, 12), b + 4, 20);
    } else {
        put(&part(36 + 4 + arm, 52, arm, 12), b + i64::from(4 - arm), 8);
        put(&part(28, 52, 4, 12), b + 4, 20);
        put(&part(32, 36, 8, 12), b + 4, 8);
        put(&part(44 + 4 + arm, 36, arm, 12), b + 12, 8);
        put(&part(52 + 4 + arm, 52, arm, 12), b + i64::from(4 - arm), 8);
        put(&part(12, 36, 4, 12), b + 8, 20);
        put(&part(12, 52, 4, 12), b + 4, 20);
    }

    let big = imageops::resize(&out, out.width() * 8, out.height() * 8, FilterType::Nearest);
    let mut bytes = std::io::Cursor::new(Vec::new());
    big.write_to(&mut bytes, image::ImageFormat::Png).ok()?;
    Some(bytes.into_inner())
}

impl SkinDialog {
    fn new(account: Account, cx: &mut Context<Self>) -> Self {
        let target = match account.kind {
            AccountKind::Microsoft => Target::Microsoft,
            AccountKind::Offline => Target::Nick,
        };
        let mut this = Self {
            account,
            target,
            current: None,
            current_slim: false,
            loading: true,
            picked: None,
            slim: false,
            library: library(),
            cloud: Vec::new(),
            busy: false,
            error: None,
        };
        this.load(cx);
        this
    }

    fn signed_in(cx: &App) -> bool {
        PlayState::global(cx).read(cx).me().is_some()
    }

    /// Whether this BX ID holds the offline account's nick, so a skin can go on it.
    fn holds(&self, cx: &App) -> bool {
        PlayState::global(cx).read(cx).owns(&self.account.name)
    }

    /// Reads what the account wears now and the skins uploaded to BX ID, keeping them in the library.
    fn load(&mut self, cx: &mut Context<Self>) {
        let signed_in = Self::signed_in(cx);
        if self.target == Target::Nick && !self.holds(cx) {
            self.loading = false;
            return;
        }
        let nick_skin = PlayState::global(cx)
            .read(cx)
            .nicks
            .as_ref()
            .and_then(|n| n.get(&self.account.name))
            .and_then(|p| p.skin.clone());
        let (account, target) = (self.account.clone(), self.target);
        let fetched = runtime::spawn(async move {
            let mine = if signed_in {
                riven_play::skins::mine()
                    .await
                    .map_err(|e| super::problem::play_error(&e))
            } else {
                Ok(Default::default())
            };
            let worn = match target {
                Target::Microsoft => {
                    async {
                        let session = riven_launch::accounts::session(&account)
                            .await
                            .map_err(|e| e.to_string())?;
                        riven_launch::accounts::current_skin(&session)
                            .await
                            .map_err(|e| e.to_string())
                    }
                    .await
                }
                Target::Nick => Ok(nick_skin.map(|s| (s.url, s.model == SkinModel::Slim))),
            };
            let worn = match worn {
                Ok(Some((url, slim))) => riven_play::skins::download(&url)
                    .await
                    .map(|png| Some((png, slim)))
                    .map_err(|e| super::problem::play_error(&e)),
                Ok(None) => Ok(None),
                Err(e) => Err(e),
            };
            let mut cloud = Vec::new();
            for skin in mine.map(|m| m.recent).unwrap_or_default() {
                if let Ok(png) = riven_play::skins::download(&skin.url).await {
                    cloud.push((png, skin.model == SkinModel::Slim));
                }
            }
            (worn, cloud)
        });
        cx.spawn(async move |this, cx| {
            let Ok((worn, cloud)) = fetched.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match worn {
                    Ok(Some((png, slim))) => match keep(&png) {
                        Ok(item) => {
                            this.current = Some(item);
                            this.current_slim = slim;
                            this.slim = slim;
                        }
                        Err(e) => this.error = Some(e.into()),
                    },
                    Ok(None) => {}
                    Err(e) => this.error = Some(e.into()),
                }
                this.cloud = cloud
                    .into_iter()
                    .filter_map(|(png, slim)| Some((keep(&png).ok()?, slim)))
                    .collect();
                this.library = library();
                cx.notify();
            });
        })
        .detach();
    }

    fn choose_file(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let Some(path) = super::pick::files(t!("skin.png_files"), &["png"], false)
                .await
                .into_iter()
                .next()
            else {
                return;
            };
            let kept = std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|png| keep(&png));
            let _ = this.update(cx, |this, cx| {
                match kept {
                    Ok(item) => {
                        this.picked = Some(item);
                        this.error = None;
                        this.library = library();
                    }
                    Err(e) => this.error = Some(e.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The skin shown and sent on Apply: the picked one, else the one worn now.
    fn shown(&self) -> Option<&Item> {
        self.picked.as_ref().or(self.current.as_ref())
    }

    fn changed(&self) -> bool {
        match (&self.picked, &self.current) {
            (Some(picked), Some(current)) if picked.hash == current.hash => {
                self.slim != self.current_slim
            }
            (Some(_), _) => true,
            (None, Some(_)) => self.slim != self.current_slim,
            (None, None) => false,
        }
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.shown().cloned() else {
            return;
        };
        let Ok(png) = std::fs::read(&item.png) else {
            return;
        };
        let (account, target, slim) = (self.account.clone(), self.target, self.slim);
        let nick = self.account.name.clone();
        let done = runtime::spawn(async move {
            match target {
                Target::Microsoft => async {
                    let session = riven_launch::accounts::session(&account).await?;
                    riven_launch::accounts::set_skin(&session, png, slim).await
                }
                .await
                .map_err(|e| e.to_string()),
                Target::Nick => {
                    let model = if slim {
                        SkinModel::Slim
                    } else {
                        SkinModel::Classic
                    };
                    riven_play::skins::upload(&nick, png, model)
                        .await
                        .map(|_| ())
                        .map_err(|e| super::problem::play_error(&e))
                }
            }
        });
        self.finish(done, t!("skin.changed"), cx);
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        let (account, target) = (self.account.clone(), self.target);
        let nick = self.account.name.clone();
        let done = runtime::spawn(async move {
            match target {
                Target::Microsoft => async {
                    let session = riven_launch::accounts::session(&account).await?;
                    riven_launch::accounts::reset_skin(&session).await
                }
                .await
                .map_err(|e| e.to_string()),
                Target::Nick => riven_play::skins::reset(&nick)
                    .await
                    .map_err(|e| super::problem::play_error(&e)),
            }
        });
        self.finish(done, t!("skin.reset_done"), cx);
    }

    /// Waits for a change, then shows the new head everywhere and closes the dialog.
    fn finish(
        &mut self,
        done: tokio::sync::oneshot::Receiver<Result<(), String>>,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.busy = true;
        self.error = None;
        let message = message.into();
        let (target, account_id) = (self.target, self.account.id.clone());
        cx.spawn(async move |this, cx| {
            let result = done.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(Ok(())) => {
                        match target {
                            Target::Microsoft => AppState::global(cx)
                                .update(cx, |app, cx| app.refresh_skin(account_id, cx)),
                            Target::Nick => {
                                PlayState::global(cx).update(cx, |s, cx| s.sync_nicks(cx))
                            }
                        }
                        moonglass::toast::show(ToastKind::Info, message, cx);
                        leave(None, cx);
                    }
                    Ok(Err(e)) => this.error = Some(e.into()),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn model_toggle(&self, cx: &mut Context<Self>) -> gpui::Div {
        let c = cx.theme().colors;
        let option = |id: &'static str, slim: bool, label: SharedString, cx: &mut Context<Self>| {
            let on = self.slim == slim;
            div()
                .id(id)
                .flex_1()
                .px(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .text_size(px(12.))
                .text_center()
                .cursor_pointer()
                .when(on, |d| d.bg(c.sel).text_color(c.text))
                .when(!on, |d| {
                    d.text_color(c.muted).hover(|s| s.text_color(c.text))
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.slim = slim;
                    cx.notify();
                }))
                .child(label)
        };
        h_flex()
            .p(px(3.))
            .gap(px(2.))
            .rounded(px(8.))
            .border_1()
            .border_color(c.border)
            .child(option("skin-classic", false, t!("skin.classic").into(), cx))
            .child(option("skin-slim", true, t!("skin.slim").into(), cx))
    }

    /// Faces to pick from; picking one with a known model switches the arms to it too.
    fn grid(&self, items: Vec<(Item, Option<bool>)>, cx: &mut Context<Self>) -> gpui::Div {
        let c = cx.theme().colors;
        let shown = self.shown().map(|i| i.hash.clone());
        h_flex()
            .flex_wrap()
            .gap(px(6.))
            .children(items.into_iter().map(|(item, slim)| {
                let on = shown.as_deref() == Some(item.hash.as_str());
                let head = item.head.clone();
                div()
                    .id(SharedString::from(format!("skin-{}", &item.hash[..16])))
                    .size(px(40.))
                    .p(px(2.))
                    .rounded(px(7.))
                    .border_2()
                    .border_color(if on { c.accent } else { c.border.opacity(0.) })
                    .cursor_pointer()
                    .hover(|s| s.border_color(c.border))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.picked = Some(item.clone());
                        if let Some(slim) = slim {
                            this.slim = slim;
                        }
                        cx.notify();
                    }))
                    .child(img(head).size_full().rounded(px(4.)))
            }))
    }
}

impl Render for SkinDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors;
        let description = match self.target {
            Target::Microsoft => t!("skin.microsoft_hint"),
            Target::Nick => t!("skin.nick_hint"),
        };
        let title = t!("skin.title", name = self.account.name.clone());

        if self.target == Target::Nick && !Self::signed_in(cx) {
            let body = div()
                .text_color(c.muted)
                .line_height(relative(1.5))
                .child(t!("skin.sign_in_first").to_string());
            let sign_in = Button::new("skin-sign-in")
                .primary()
                .size(ButtonSize::Md)
                .label(t!("bxid.sign_in"))
                .on_click(|_, _, cx| super::play::open_sign_in(None, cx));
            return dialog_shell(title, description, body, vec![cancel(), sign_in], cx);
        }
        if self.target == Target::Nick && !self.holds(cx) {
            let name = self.account.name.clone();
            let why = if PlayState::global(cx).read(cx).is_taken(&name) {
                t!("skin.nick_taken", name = name)
            } else {
                t!("skin.nick_not_held", name = name)
            };
            let body = div()
                .text_color(c.muted)
                .line_height(relative(1.5))
                .child(why.to_string());
            return dialog_shell(title, description, body, vec![cancel()], cx);
        }

        let picture = self.shown().and_then(|item| preview(item, self.slim));
        let cloud: Vec<(Item, Option<bool>)> = self
            .cloud
            .iter()
            .map(|(item, slim)| (item.clone(), Some(*slim)))
            .collect();
        // What is on BX ID is listed there; this computer's list keeps the rest.
        let local: Vec<(Item, Option<bool>)> = self
            .library
            .iter()
            .filter(|item| !self.cloud.iter().any(|(c, _)| c.hash == item.hash))
            .map(|item| (item.clone(), None))
            .collect();
        let stage = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(220.))
            .h(px(220.))
            .rounded(px(10.))
            .border_1()
            .border_color(c.border)
            .bg(c.row)
            .map(|d| match picture {
                // 36×32 pixels of skin scaled ×8; shown at 3/4 so it fits the stage.
                Some(path) => d.child(img(path).w(px(216.)).h(px(192.))),
                None => d.child(div().text_size(px(12.)).text_color(c.muted).child(
                    if self.loading {
                        t!("skin.loading").to_string()
                    } else {
                        t!("skin.none").to_string()
                    },
                )),
            });

        let side = v_flex()
            .flex_1()
            .min_w_0()
            .gap(px(12.))
            .child(caption(t!("skin.arms").into(), cx))
            .child(self.model_toggle(cx))
            .child(
                Button::new("skin-file")
                    .outline()
                    .size(ButtonSize::Md)
                    .label(t!("skin.choose_file"))
                    .on_click(cx.listener(|this, _, _, cx| this.choose_file(cx))),
            )
            .when(!cloud.is_empty(), |d| {
                d.child(caption(t!("skin.cloud").into(), cx))
                    .child(self.grid(cloud, cx))
            })
            .when(!local.is_empty(), |d| {
                d.child(caption(t!("skin.library").into(), cx))
                    .child(self.grid(local, cx))
            })
            .when_some(self.error.clone(), |d, error| {
                d.child(super::problem::inline(error, cx))
            });

        let body = h_flex().items_start().gap(px(18.)).child(stage).child(side);
        let reset = Button::new("skin-reset")
            .ghost()
            .size(ButtonSize::Md)
            .label(match self.target {
                Target::Microsoft => t!("skin.reset_microsoft"),
                Target::Nick => t!("skin.reset_nick"),
            })
            .disabled(self.busy || self.loading)
            .on_click(cx.listener(|this, _, _, cx| this.reset(cx)));
        let apply = Button::new("skin-apply")
            .primary()
            .size(ButtonSize::Md)
            .label(t!("skin.apply"))
            .loading(self.busy)
            .disabled(!self.changed() || self.busy)
            .on_click(cx.listener(|this, _, _, cx| this.apply(cx)));
        dialog_shell(title, description, body, vec![reset, cancel(), apply], cx)
    }
}

fn cancel() -> Button {
    Button::new("skin-cancel")
        .outline()
        .size(ButtonSize::Md)
        .label(t!("common.cancel"))
        .on_click(|_, _, cx| leave(None, cx))
}

fn caption(text: SharedString, cx: &App) -> gpui::Div {
    div()
        .text_size(px(11.))
        .text_color(cx.theme().colors.muted)
        .child(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 64×`h` skin where every pixel's colour names the texture point it came from.
    fn coded(h: u32) -> Vec<u8> {
        let skin = image::RgbaImage::from_fn(64, h, |x, y| image::Rgba([x as u8, y as u8, 7, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        skin.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn at(png: &[u8], x: u32, y: u32) -> [u8; 4] {
        // Rendered pixels are 8×8 blocks; sample a block's middle.
        image::load_from_memory(png)
            .unwrap()
            .to_rgba8()
            .get_pixel(x * 8 + 4, y * 8 + 4)
            .0
    }

    #[test]
    fn slim_arms_come_from_three_pixel_columns_and_legacy_skins_mirror_the_left_side() {
        let modern = render(&coded(64), true).unwrap();
        // Front right arm, slim: texture x 44..47 drawn at x 1..4; the sleeve layer (y 36) covers it.
        assert_eq!(at(&modern, 1, 8), [44, 36, 7, 255]);
        // Back right arm, slim: starts at x 44 + 4 + 3 = 51.
        assert_eq!(at(&modern, 32, 8), [51, 36, 7, 255]);
        let legacy = render(&coded(32), false).unwrap();
        // A 64×32 skin has no left arm: the right one (x 44..48) is flipped, so x 15 shows texture x 44.
        assert_eq!(at(&legacy, 15, 8), [44, 20, 7, 255]);
    }
}
