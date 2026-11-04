/// Where an instance's game comes from, as the launch bar describes it.
pub struct Instance {
    pub name: &'static str,
    pub minecraft: &'static str,
    pub loader: Option<(&'static str, &'static str)>,
    /// The installed pack version, for an instance that follows a pack.
    pub pack: Option<&'static str>,
    pub played: &'static str,
    pub mods: &'static [Item],
    pub resource_packs: &'static [Item],
    pub shaders: &'static [Item],
    pub worlds: &'static [World],
}

/// A file in one of the content tabs: a mod, a resource pack or a shader pack.
#[derive(Clone)]
pub struct Item {
    pub name: &'static str,
    /// The project icon on Modrinth's CDN, which the real launcher shows too.
    pub icon: Option<&'static str>,
    pub version: &'static str,
    pub modified: &'static str,
    /// Installed by the pack, so the player cannot switch or remove it.
    pub from_pack: bool,
}

const fn pack(name: &'static str, icon: Option<&'static str>, version: &'static str) -> Item {
    Item {
        name,
        icon,
        version,
        modified: "4 days ago",
        from_pack: true,
    }
}

const fn own(
    name: &'static str,
    icon: Option<&'static str>,
    version: &'static str,
    modified: &'static str,
) -> Item {
    Item {
        name,
        icon,
        version,
        modified,
        from_pack: false,
    }
}

/// The content of Vanilla Reimagined, the BX Team pack, as it is published.
const REIMAGINED: &[Item] = &[
    pack("BadOptimizations", None, "2.4.1"),
    pack(
        "Cloth Config API",
        Some(
            "https://cdn.modrinth.com/data/9s6osm5g/ed8a2316cbb6f4fc5f510e8e13a59a85cbbbff4d_96.webp",
        ),
        "26.3.159",
    ),
    pack(
        "Continuity",
        Some("https://cdn.modrinth.com/data/1IjD5062/icon.png"),
        "3.0.1",
    ),
    pack(
        "Dynamic FPS",
        Some(
            "https://cdn.modrinth.com/data/LQ3K71Q1/5056368d0d87c1a9f3efead0cb48ab39a4ea87bf_96.webp",
        ),
        "3.11.10",
    ),
    pack(
        "[EMF] Entity Model Features",
        Some(
            "https://cdn.modrinth.com/data/4I1XuqiY/6e5bbd0d06b1741bfdab6c0cfab6de8fdaf0064c_96.webp",
        ),
        "3.3.12",
    ),
    pack(
        "Entity Culling",
        Some(
            "https://cdn.modrinth.com/data/NNAgCjsB/7873452d6cede4daed12da3d7d8c193ab88b4fd6_96.webp",
        ),
        "1.11.3",
    ),
    pack(
        "[ETF] Entity Texture Features",
        Some(
            "https://cdn.modrinth.com/data/BVzZfTc1/af683d206d50b05258d865b0d6e4aa2f2cee12f2_96.webp",
        ),
        "7.2.6",
    ),
    pack(
        "Fabric API",
        Some("https://cdn.modrinth.com/data/P7dR8mSH/icon.png"),
        "0.165.0",
    ),
    pack(
        "Fabric Language Kotlin",
        Some(
            "https://cdn.modrinth.com/data/Ha28R6CL/72c3d74aeb665e45aea93a945a01474cbce3b7da_96.webp",
        ),
        "1.14.2",
    ),
    pack(
        "FerriteCore",
        Some(
            "https://cdn.modrinth.com/data/uXXizFIs/222a126f26f8f9ae1eb339f3b767677f18bff31f_96.webp",
        ),
        "9.0.0",
    ),
    pack(
        "Forge Config API Port",
        Some(
            "https://cdn.modrinth.com/data/ohNO6lps/cb3b942cc18a66a0f35f802e004713f134e46cc2_96.webp",
        ),
        "26.3.1",
    ),
    pack(
        "ImmediatelyFast",
        Some(
            "https://cdn.modrinth.com/data/5ZwdcRci/e57b6b451425692ac17ad322d5e14bea686a383a_96.webp",
        ),
        "1.17.1",
    ),
    pack(
        "Iris Shaders",
        Some(
            "https://cdn.modrinth.com/data/YL57xq9U/18d0e7f076d3d6ed5bedd472b853909aac5da202_96.webp",
        ),
        "1.11.7",
    ),
    pack(
        "LambDynamicLights",
        Some(
            "https://cdn.modrinth.com/data/yBW8D80W/d4f5c3ff8df7caf024178b04eca6d69f95979cfe_96.webp",
        ),
        "4.13.0",
    ),
    pack(
        "Lithium",
        Some(
            "https://cdn.modrinth.com/data/gvQqBUqZ/bcc8686c13af0143adf4285d741256af824f70b7_96.webp",
        ),
        "0.26.2",
    ),
    pack(
        "Mod Menu",
        Some("https://cdn.modrinth.com/data/mOgUt4GM/5a20ed1450a0e1e79a1fe04e61bb4e5878bf1d20.png"),
        "21.0.0",
    ),
    pack(
        "Moonrise",
        Some(
            "https://cdn.modrinth.com/data/KOHu7RCS/98af0394452604fc05adf4f851f1216204e03a40_96.webp",
        ),
        "1.2.1",
    ),
    pack(
        "More Culling",
        Some(
            "https://cdn.modrinth.com/data/51shyZVL/c51b07193b56e952269ef50101d12aecba2b4747_96.webp",
        ),
        "1.9.0",
    ),
    pack(
        "OptiGUI",
        Some(
            "https://cdn.modrinth.com/data/JuksLGBQ/5e7e96524f7a7bcd8fb8936da34f8a26fb5c6a41_96.webp",
        ),
        "2.3.0-beta.10",
    ),
    pack(
        "Text Placeholder API",
        Some("https://cdn.modrinth.com/data/eXts2L7r/e9c9990896e6422bffc5f73d2c41b8f077348f83.png"),
        "3.2.0",
    ),
    pack(
        "Puzzle",
        Some(
            "https://cdn.modrinth.com/data/3IuO68q1/7f24e545ac7779557dfffb89b6e0981d0c2d4a7e_96.webp",
        ),
        "2.3.1",
    ),
    pack(
        "Reese's Sodium Options",
        Some("https://cdn.modrinth.com/data/Bh37bMuy/icon.png"),
        "2.2.6-beta.1",
    ),
    pack(
        "Remove Reloading Screen",
        Some(
            "https://cdn.modrinth.com/data/ZP7xHXtw/7a416b09817a79adfa6a3ef9368990135e276821_96.webp",
        ),
        "5.2.8",
    ),
    pack(
        "Sodium",
        Some(
            "https://cdn.modrinth.com/data/AANobbMI/295862f4724dc3f78df3447ad6072b2dcd3ef0c9_96.webp",
        ),
        "0.9.3-beta.1",
    ),
    pack(
        "Sodium Extra",
        Some(
            "https://cdn.modrinth.com/data/PtjYWJkn/0df4fb22a11e1dcb5e83cb0aadd275b571aca7a9_96.webp",
        ),
        "0.9.6-beta.1",
    ),
    pack(
        "YetAnotherConfigLib",
        Some(
            "https://cdn.modrinth.com/data/1eAoo2KR/08c0cd32515e260f4bb20bbc0696510041523f9a_96.webp",
        ),
        "3.9.7",
    ),
    pack(
        "Zoomify",
        Some(
            "https://cdn.modrinth.com/data/w7ThoJFB/e2de67a0bfb9e8aa2347982ab3ec5463f26cca31_96.webp",
        ),
        "2.17.0",
    ),
    own(
        "Xaero's Minimap",
        Some(
            "https://cdn.modrinth.com/data/1bokaNcj/354080f65407e49f486fcf9c4580e82c45ae63b8_96.webp",
        ),
        "25.3.4",
        "yesterday",
    ),
    own(
        "AppleSkin",
        Some("https://cdn.modrinth.com/data/EsAfCjCV/icon.png"),
        "3.0.7",
        "2 weeks ago",
    ),
];

const FRIENDS: &[Item] = &[
    own(
        "Fabric API",
        Some("https://cdn.modrinth.com/data/P7dR8mSH/icon.png"),
        "0.165.0",
        "3 weeks ago",
    ),
    own(
        "Sodium",
        Some(
            "https://cdn.modrinth.com/data/AANobbMI/295862f4724dc3f78df3447ad6072b2dcd3ef0c9_96.webp",
        ),
        "0.9.3-beta.1",
        "3 weeks ago",
    ),
    own(
        "Lithium",
        Some(
            "https://cdn.modrinth.com/data/gvQqBUqZ/bcc8686c13af0143adf4285d741256af824f70b7_96.webp",
        ),
        "0.26.2",
        "3 weeks ago",
    ),
    own(
        "Simple Voice Chat",
        Some("https://cdn.modrinth.com/data/9eGKb6K1/icon.png"),
        "2.6.4",
        "3 weeks ago",
    ),
    own(
        "Jade",
        Some(
            "https://cdn.modrinth.com/data/nvQzSEkH/b04217bc2b7dc524c4d12f81ff42cc1cefb9b0fc_96.webp",
        ),
        "21.2.0",
        "2 weeks ago",
    ),
    own(
        "Mouse Tweaks",
        Some(
            "https://cdn.modrinth.com/data/aC3cM3Vq/6c0eaa4e60a9c87f4766f222ff63286f09da32c0_96.webp",
        ),
        "2.27",
        "2 weeks ago",
    ),
    own(
        "Xaero's World Map",
        Some(
            "https://cdn.modrinth.com/data/NcUtCpym/354080f65407e49f486fcf9c4580e82c45ae63b8_96.webp",
        ),
        "1.40.1",
        "6 days ago",
    ),
];

const FRESH_ANIMATIONS: &str =
    "https://cdn.modrinth.com/data/50dA9Sha/3132c10e9e3c73fde9799720fd3da5561071708c_96.webp";
const ENCHANTED_BOOKS: &str =
    "https://cdn.modrinth.com/data/ZpBKASR2/6c2d9ce61b9356e67ec6decdad8e828e77718132.png";
const LOW_ON_FIRE: &str =
    "https://cdn.modrinth.com/data/RRxvWKNC/06a7e7691a6da41798c255108a8563d7ffea171d.png";
const DARK_MODE: &str =
    "https://cdn.modrinth.com/data/6SLU7tS5/f90001e4ebed860e7ae913f34c85d745f516a89b.png";
const FAITHFUL: &str =
    "https://cdn.modrinth.com/data/w0TnApzs/e8403d1fb2f55321ae74402c1e8c90a3a5670856.png";
const BETTER_GRASS: &str =
    "https://cdn.modrinth.com/data/dspVZXKP/5de0774f423d220fcd4e635b91f2bdfcb8a2b910.png";
const BARE_BONES: &str =
    "https://cdn.modrinth.com/data/rox3U8B6/b502bd8b2a6048f635230da8e8a66eeb4383874b_96.webp";
const NAUTILUS: &str =
    "https://cdn.modrinth.com/data/1MyOrwfu/7c09c9751c730404ae5aef448183b4e3ccedd24a_96.webp";
const REIMAGINED_SHADER: &str =
    "https://cdn.modrinth.com/data/HVnmMxH1/79cb7c8123bbc54945305b2ebad6b8881efdf5f8_96.webp";
const UNBOUND: &str =
    "https://cdn.modrinth.com/data/R6NEzAwj/c85ce4049aac76360d2cd24fd9a7003de01ef312_96.webp";
const SOLAS: &str =
    "https://cdn.modrinth.com/data/EpQFjzrQ/e3efc6ba7a63f9e1cf473a794d0224a6daf243c7_96.webp";
const BSL: &str =
    "https://cdn.modrinth.com/data/Q1vvjJYV/2a611a3cb434fb52fb81fa5dace13c5d8b67e55d_96.webp";
const PHOTON: &str =
    "https://cdn.modrinth.com/data/lLqFfGNs/39cb5f12e7dcc68d6cb666f225fcb2b801dd70fb_96.webp";

const REIMAGINED_PACKS: &[Item] = &[
    own(
        "Fresh Animations",
        Some(FRESH_ANIMATIONS),
        "1.10.2",
        "yesterday",
    ),
    own(
        "xali's Enchanted Books",
        Some(ENCHANTED_BOOKS),
        "1.4.1",
        "5 days ago",
    ),
    own("Low On Fire", Some(LOW_ON_FIRE), "1.2", "2 weeks ago"),
];

const REIMAGINED_SHADERS: &[Item] = &[
    own(
        "Complementary Shaders - Reimagined",
        Some(REIMAGINED_SHADER),
        "r5.6.1",
        "yesterday",
    ),
    own("Solas Shader", Some(SOLAS), "3.6", "3 weeks ago"),
];

const FRIENDS_PACKS: &[Item] = &[
    own("Faithful 32x", Some(FAITHFUL), "26.3", "3 weeks ago"),
    own(
        "Fast Better Grass",
        Some(BETTER_GRASS),
        "1.6",
        "3 weeks ago",
    ),
];

const VANILLA_PACKS: &[Item] = &[own("Bare Bones", Some(BARE_BONES), "26.3", "today")];

/// A save in the Worlds tab.
pub struct World {
    pub name: &'static str,
    pub mode: &'static str,
    pub difficulty: &'static str,
    pub day: u32,
    pub played: &'static str,
    pub size: &'static str,
    pub backups: u32,
}

const REIMAGINED_WORLDS: &[World] = &[
    World {
        name: "New World",
        mode: "Survival",
        difficulty: "Normal",
        day: 214,
        played: "yesterday",
        size: "186 MB",
        backups: 3,
    },
    World {
        name: "Shader screenshots",
        mode: "Creative",
        difficulty: "Peaceful",
        day: 12,
        played: "2 weeks ago",
        size: "42 MB",
        backups: 0,
    },
];

const FRIENDS_WORLDS: &[World] = &[World {
    name: "Island SMP",
    mode: "Hardcore",
    difficulty: "Hard",
    day: 57,
    played: "6 days ago",
    size: "94 MB",
    backups: 1,
}];

/// A project the Add dialog finds on "Modrinth".
pub struct Hit {
    pub name: &'static str,
    pub icon: &'static str,
    pub summary: &'static str,
    pub downloads: &'static str,
    pub version: &'static str,
}

const fn hit(
    name: &'static str,
    icon: &'static str,
    summary: &'static str,
    downloads: &'static str,
    version: &'static str,
) -> Hit {
    Hit {
        name,
        icon,
        summary,
        downloads,
        version,
    }
}

pub const MOD_HITS: &[Hit] = &[
    hit(
        "Sodium",
        "https://cdn.modrinth.com/data/AANobbMI/295862f4724dc3f78df3447ad6072b2dcd3ef0c9_96.webp",
        "A high-performance rendering engine replacement for Minecraft.",
        "249.9M",
        "0.9.3-beta.1",
    ),
    hit(
        "Iris Shaders",
        "https://cdn.modrinth.com/data/YL57xq9U/18d0e7f076d3d6ed5bedd472b853909aac5da202_96.webp",
        "A modern shader pack loader, compatible with OptiFine shader packs.",
        "193.3M",
        "1.11.7",
    ),
    hit(
        "Entity Culling",
        "https://cdn.modrinth.com/data/NNAgCjsB/7873452d6cede4daed12da3d7d8c193ab88b4fd6_96.webp",
        "Hides block entities and entities that are not visible.",
        "186.5M",
        "1.11.3",
    ),
    hit(
        "Xaero's Minimap",
        "https://cdn.modrinth.com/data/1bokaNcj/354080f65407e49f486fcf9c4580e82c45ae63b8_96.webp",
        "A map of the nearby terrain in the corner of the screen, with waypoints.",
        "119.7M",
        "25.3.4",
    ),
    hit(
        "AppleSkin",
        "https://cdn.modrinth.com/data/EsAfCjCV/icon.png",
        "Food and hunger improvements for the HUD.",
        "98.2M",
        "3.0.7",
    ),
    hit(
        "Just Enough Items (JEI)",
        "https://cdn.modrinth.com/data/u6dRKJwZ/4a3f18ac0d096c9f8e9176984c44be4e58f94c89_96.webp",
        "View items and recipes.",
        "84.9M",
        "26.3.0.12",
    ),
    hit(
        "Simple Voice Chat",
        "https://cdn.modrinth.com/data/9eGKb6K1/icon.png",
        "A working voice chat in Minecraft.",
        "76.0M",
        "2.6.4",
    ),
    hit(
        "Jade",
        "https://cdn.modrinth.com/data/nvQzSEkH/b04217bc2b7dc524c4d12f81ff42cc1cefb9b0fc_96.webp",
        "Shows information about what you are looking at.",
        "73.5M",
        "21.2.0",
    ),
    hit(
        "Zoomify",
        "https://cdn.modrinth.com/data/w7ThoJFB/e2de67a0bfb9e8aa2347982ab3ec5463f26cca31_96.webp",
        "A zoom mod with infinite customizability.",
        "72.0M",
        "2.17.0",
    ),
    hit(
        "Mouse Tweaks",
        "https://cdn.modrinth.com/data/aC3cM3Vq/6c0eaa4e60a9c87f4766f222ff63286f09da32c0_96.webp",
        "Inventory management with more mouse button functions.",
        "64.1M",
        "2.27",
    ),
];

pub const PACK_HITS: &[Hit] = &[
    hit(
        "Fresh Animations",
        FRESH_ANIMATIONS,
        "Dynamic animated entities, like in the trailers.",
        "51.6M",
        "1.10.2",
    ),
    hit(
        "Low On Fire",
        LOW_ON_FIRE,
        "Low fire on your screen, vanilla friendly.",
        "17.4M",
        "1.2",
    ),
    hit(
        "Fast Better Grass",
        BETTER_GRASS,
        "Grass blocks use the top texture on their sides too.",
        "14.0M",
        "1.6",
    ),
    hit(
        "Default Dark Mode",
        DARK_MODE,
        "The dark mode resource pack for Minecraft: Java Edition.",
        "8.3M",
        "26.3",
    ),
    hit(
        "Bare Bones",
        BARE_BONES,
        "Minecraft in the style of the trailers.",
        "8.0M",
        "26.3",
    ),
    hit(
        "Faithful 32x",
        FAITHFUL,
        "The original texture feel at double the resolution.",
        "5.3M",
        "26.3",
    ),
    hit(
        "Nautilus 3D",
        NAUTILUS,
        "3D models in a vanilla style.",
        "1.6M",
        "4.2",
    ),
    hit(
        "xali's Enchanted Books",
        ENCHANTED_BOOKS,
        "Custom textures that tell enchanted books apart.",
        "498K",
        "1.4.1",
    ),
];

pub const SHADER_HITS: &[Hit] = &[
    hit(
        "Complementary Shaders - Reimagined",
        REIMAGINED_SHADER,
        "Preserving the elements of Minecraft with quality, detail and performance.",
        "72.2M",
        "r5.6.1",
    ),
    hit(
        "Complementary Shaders - Unbound",
        UNBOUND,
        "Transforming the visuals of Minecraft with quality, detail and performance.",
        "46.9M",
        "r5.6.1",
    ),
    hit(
        "BSL Shaders",
        BSL,
        "Bright, colorful and distinct.",
        "31.6M",
        "10.1",
    ),
    hit(
        "Photon Shaders",
        PHOTON,
        "A gameplay-focused shader pack with a semi-realistic style.",
        "29.1M",
        "1.2",
    ),
    hit(
        "Solas Shader",
        SOLAS,
        "A modern fantasy stylized shader pack with colored lighting.",
        "18.6M",
        "3.6",
    ),
];

/// The Modrinth page of a project, from the project id in its CDN icon URL.
pub fn project_page(icon: &str) -> Option<String> {
    let id = icon
        .strip_prefix("https://cdn.modrinth.com/data/")?
        .split('/')
        .next()?;
    Some(format!("https://modrinth.com/project/{id}"))
}

impl Item {
    /// A file the player just added from the Add dialog.
    pub fn added(hit: &Hit) -> Self {
        Self {
            name: hit.name,
            icon: Some(hit.icon),
            version: hit.version,
            modified: "just now",
            from_pack: false,
        }
    }
}

pub const INSTANCES: &[Instance] = &[
    Instance {
        name: "Vanilla Reimagined",
        minecraft: "26.3",
        loader: Some(("Fabric", "0.20.1")),
        pack: Some("26.3-1.0.1"),
        played: "12 h 40 min played · yesterday",
        mods: REIMAGINED,
        resource_packs: REIMAGINED_PACKS,
        shaders: REIMAGINED_SHADERS,
        worlds: REIMAGINED_WORLDS,
    },
    Instance {
        name: "Survival with friends",
        minecraft: "26.3",
        loader: Some(("Fabric", "0.20.1")),
        pack: None,
        played: "3 h 5 min played · 6 days ago",
        mods: FRIENDS,
        resource_packs: FRIENDS_PACKS,
        shaders: &[],
        worlds: FRIENDS_WORLDS,
    },
    Instance {
        name: "Vanilla",
        minecraft: "26.3",
        loader: None,
        pack: None,
        played: "Never played",
        mods: &[],
        resource_packs: VANILLA_PACKS,
        shaders: &[],
        worlds: &[],
    },
];

/// What a launch prints, a few lines at a time, once the game is running.
pub const LOG: &[&str] = &[
    "[Render thread/INFO]: Environment: Environment[sessionHost=https://sessionserver.mojang.com]",
    "[Render thread/INFO]: Setting user: Steve",
    "[Render thread/INFO]: Backend library: LWJGL version 3.3.6",
    "[Render thread/INFO]: Loading 29 mods",
    "[Render thread/INFO]: [Sodium] Loaded configuration file",
    "[Render thread/INFO]: [Iris] Shaders are disabled",
    "[Render thread/INFO]: Reloading ResourceManager: vanilla, fabric, continuity",
    "[Worker-Main-2/INFO]: Found unifont_all_no_pua-16.0.03.hex, loading",
    "[Render thread/INFO]: OpenAL initialized on device Default",
    "[Render thread/INFO]: Sound engine started",
    "[Render thread/INFO]: Created: 2048x1024x4 minecraft:textures/atlas/blocks.png-atlas",
    "[Render thread/INFO]: Loaded 1402 advancements",
    "[Server thread/INFO]: Preparing level \"New World\"",
    "[Server thread/INFO]: Done (2.481s)! For help, type \"help\"",
];

/// The steps a launch reports before the game window opens.
pub const STEPS: &[&str] = &[
    "Checking the pack",
    "Downloading 3 updated files",
    "Checking Java 25",
    "Preparing assets",
    "Starting Minecraft",
];
