use std::collections::BTreeMap;
use std::sync::OnceLock;

use sha2::{Digest, Sha256};

use crate::{
    AccentRoles, Appearance, Color, SurfaceTreatment, TerminalPalette, ThemeColors, ThemeFamily,
    ThemeRegistry, ThemeSource, ThemeVariant,
};

pub fn builtin_registry() -> &'static ThemeRegistry {
    static REGISTRY: OnceLock<ThemeRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| ThemeRegistry {
        families: vec![
            family("zuse", "Zuse", vec![zuse_light(), zuse_dark()]),
            family(
                "glass",
                "Glass",
                vec![glass(Appearance::Light), glass(Appearance::Dark)],
            ),
            family(
                "observatory",
                "Observatory",
                vec![observatory_light(), observatory_dark()],
            ),
            family("flexoki", "Flexoki", vec![flexoki_light(), flexoki_dark()]),
            family(
                "everforest",
                "Everforest",
                vec![everforest_light(), everforest_dark()],
            ),
            family(
                "catppuccin",
                "Catppuccin",
                vec![catppuccin_latte(), catppuccin_mocha()],
            ),
            family(
                "tokyo-night",
                "Tokyo Night",
                vec![tokyo_light(), tokyo_dark()],
            ),
            family("dracula", "Dracula", vec![dracula()]),
            family("github", "GitHub", vec![github_light(), github_dark()]),
            family("ayu", "Ayu", vec![ayu_light(), ayu_dark()]),
            family("gruvbox", "Gruvbox", vec![gruvbox_light(), gruvbox_dark()]),
            family(
                "rose-pine",
                "Rosé Pine",
                vec![rose_pine_dawn(), rose_pine_moon()],
            ),
            family("nord", "Nord", vec![nord()]),
            family("one-dark-pro", "One Dark Pro", vec![one_dark_pro()]),
        ],
    })
}

fn family(id: &str, name: &str, variants: Vec<ThemeVariant>) -> ThemeFamily {
    ThemeFamily {
        id: id.into(),
        name: name.into(),
        variants,
    }
}

struct Seeds<'a> {
    id: &'a str,
    family_id: &'a str,
    name: &'a str,
    appearance: Appearance,
    treatment: SurfaceTreatment,
    background: &'a str,
    shell: &'a str,
    raised: &'a str,
    card: &'a str,
    text: &'a str,
    muted: &'a str,
    faint: &'a str,
    accent: &'a str,
    danger: &'a str,
    warning: &'a str,
    success: &'a str,
    terminal_background: &'a str,
    ansi: [&'a str; 16],
    syntax: [&'a str; 12],
    source: ThemeSource,
}

fn variant(seed: Seeds<'_>) -> ThemeVariant {
    let background = c(seed.background);
    let shell = c(seed.shell);
    let raised = c(seed.raised);
    let card = c(seed.card);
    let text = c(seed.text);
    let muted = c(seed.muted).ensure_contrast(background, 4.5);
    let faint = c(seed.faint);
    let danger = c(seed.danger);
    let warning = c(seed.warning);
    let success = c(seed.success);
    let accent = AccentRoles::derive(c(seed.accent), seed.appearance, background);
    let dark = seed.appearance.is_dark();
    let border_tone = if dark { Color::WHITE } else { Color::BLACK };
    let solid = if dark {
        Color::rgb(235, 235, 239)
    } else {
        Color::rgb(35, 35, 40)
    };
    let colors = ThemeColors {
        background,
        shell,
        raised,
        card,
        dialog: card.mix(raised, if dark { 0.18 } else { 0.04 }),
        overlay: card.mix(raised, if dark { 0.34 } else { 0.02 }),
        hover: border_tone.with_alpha(if dark { 0.11 } else { 0.06 }),
        active: accent.primary.with_alpha(if dark { 0.18 } else { 0.10 }),
        selected: ThemeColors::derive_selected(background),
        border: border_tone.with_alpha(if dark { 0.10 } else { 0.12 }),
        border_strong: border_tone.with_alpha(if dark { 0.18 } else { 0.22 }),
        text_strong: ThemeColors::derive_text_strong(text, background),
        text,
        text_muted: muted,
        text_faint: faint,
        solid,
        on_solid: solid.best_on_color(),
        danger,
        danger_muted: danger.mix(text, 0.28),
        warning,
        warning_muted: warning.mix(text, 0.25),
        success,
        success_muted: success.mix(text, 0.25),
        input: if dark { raised.with_alpha(0.72) } else { card },
        cursor: text.with_alpha(if dark { 0.40 } else { 0.55 }),
        diff_add: success,
        diff_delete: danger,
        diff_hunk: accent.primary.with_alpha(if dark { 0.08 } else { 0.07 }),
    };
    let terminal_background = c(seed.terminal_background);
    let mut variant = ThemeVariant {
        id: seed.id.into(),
        family_id: seed.family_id.into(),
        name: seed.name.into(),
        appearance: seed.appearance,
        recommended_surface_treatment: seed.treatment,
        colors,
        accent,
        syntax: syntax(seed.syntax),
        terminal: TerminalPalette {
            background: terminal_background,
            foreground: text.ensure_contrast(terminal_background, 4.5),
            selection: border_tone.with_alpha(if dark { 0.22 } else { 0.16 }),
            ansi: seed.ansi.map(c),
        },
        source: seed.source,
    };
    // Hash the checked-in resolved definition itself (with the hash field
    // blanked), not merely its source URL. This makes provenance sensitive to
    // curation edits as well as upstream revision changes.
    variant.source.asset_hash.clear();
    let encoded = serde_json::to_vec(&variant).expect("built-in theme serializes");
    variant.source.asset_hash = format!("sha256:{:x}", Sha256::digest(encoded));
    variant
}

fn syntax(colors: [&str; 12]) -> BTreeMap<String, Color> {
    let [
        comment,
        keyword,
        string,
        number,
        type_name,
        function,
        property,
        variable,
        punctuation,
        tag,
        attribute,
        invalid,
    ] = colors.map(c);
    BTreeMap::from([
        ("comment".into(), comment),
        ("keyword".into(), keyword),
        ("string".into(), string),
        ("stringSpecial".into(), attribute),
        ("escape".into(), attribute),
        ("number".into(), number),
        ("boolean".into(), number),
        ("type".into(), type_name),
        ("typeBuiltin".into(), type_name),
        ("constructor".into(), type_name),
        ("function".into(), function),
        ("functionBuiltin".into(), function),
        ("macro".into(), keyword),
        ("property".into(), property),
        ("constant".into(), number),
        ("variable".into(), variable),
        ("variableSpecial".into(), keyword),
        ("parameter".into(), variable),
        ("operator".into(), keyword),
        ("punctuation".into(), punctuation),
        ("tag".into(), tag),
        ("attribute".into(), attribute),
        ("label".into(), function),
        ("embedded".into(), punctuation),
        ("invalid".into(), invalid),
    ])
}

fn source(id: &str, format: &str, url: &str, revision: &str, license: &str) -> ThemeSource {
    ThemeSource {
        format: format.into(),
        url: url.into(),
        revision: revision.into(),
        license: license.into(),
        asset_hash: format!("pending:{id}"),
    }
}

fn c(value: &str) -> Color {
    value.parse().expect("built-in theme colors are valid")
}

const ANSI_DARK: [&str; 16] = [
    "#242424", "#f87171", "#4ade80", "#facc15", "#60a5fa", "#c084fc", "#22d3ee", "#d4d4d8",
    "#52525b", "#fca5a5", "#86efac", "#fde047", "#93c5fd", "#d8b4fe", "#67e8f9", "#fafafa",
];

const ANSI_LIGHT: [&str; 16] = [
    "#1f1f1f", "#dc2626", "#16a34a", "#b45309", "#2563eb", "#9333ea", "#0e7490", "#3f3f46",
    "#71717a", "#b91c1c", "#15803d", "#92400e", "#1d4ed8", "#7e22ce", "#155e75", "#18181b",
];

fn zuse_dark() -> ThemeVariant {
    let mut palette = variant(Seeds {
        id: "zuse-dark",
        family_id: "zuse",
        name: "Zuse Dark",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#151515",
        shell: "#1e1e1e",
        raised: "#222222",
        card: "#181818",
        text: "#e8e8e8",
        muted: "#a3a3a3",
        faint: "#6e6e6e",
        accent: "#89ca21",
        danger: "#f87171",
        warning: "#facc15",
        success: "#34d399",
        terminal_background: "#121212",
        ansi: ANSI_DARK,
        // The reference renderer's code palette (comment, keyword, string,
        // number, type, function, property, variable, punctuation, tag,
        // attribute, invalid): cyan types and sky functions over the
        // neutral greys, plain text in the foreground tone.
        syntax: [
            "#a3a3a3", "#c084fc", "#86efac", "#fbbf24", "#67e8f9", "#7dd3fc", "#e8e8e8", "#e8e8e8",
            "#a3a3a3", "#f87171", "#fda4af", "#f87171",
        ],
        source: source(
            "zuse-dark",
            "native",
            "https://github.com/opcodehq/zuse",
            "d138049",
            "MIT",
        ),
    });
    zuse_code_roles(&mut palette);
    palette.colors.solid = c("#e8e8e8");
    palette.colors.on_solid = palette.colors.solid.best_on_color();
    palette.colors.border = c("#272727");
    palette.colors.border_strong = c("#323232");
    palette.colors.input = c("#262626");
    palette.accent.primary = c("#89ca21");
    palette.accent.strong = c("#89ca21");
    palette.accent.on = c("#0c170f");
    palette
}

/// Zuse code follows the reference renderer's token roles: operators read
/// as punctuation (muted), parameters as attributes, and `self`-style
/// variables as plain text rather than keyword-tinted.
fn zuse_code_roles(palette: &mut ThemeVariant) {
    let punctuation = palette.syntax["punctuation"];
    let attribute = palette.syntax["attribute"];
    let variable = palette.syntax["variable"];
    palette.syntax.insert("operator".into(), punctuation);
    palette.syntax.insert("parameter".into(), attribute);
    palette.syntax.insert("variableSpecial".into(), variable);
}

fn zuse_light() -> ThemeVariant {
    let mut palette = variant(Seeds {
        id: "zuse-light",
        family_id: "zuse",
        name: "Zuse Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#ffffff",
        shell: "#efefef",
        raised: "#efefef",
        card: "#ffffff",
        text: "#2a2a2a",
        muted: "#737373",
        faint: "#a3a3a3",
        accent: "#81bf1d",
        danger: "#dc2626",
        warning: "#a16207",
        success: "#15803d",
        terminal_background: "#fafafa",
        ansi: ANSI_LIGHT,
        // The reference renderer's light code palette, with string, number
        // and type deepened a step to hold 4.5:1 on white and diff rows.
        syntax: [
            "#7f7f7f", "#7924b2", "#297537", "#90600e", "#186f81", "#2256bf", "#292929", "#292929",
            "#7f7f7f", "#c32222", "#ba2157", "#b91c1c",
        ],
        source: source(
            "zuse-light",
            "native",
            "https://github.com/opcodehq/zuse",
            "d138049",
            "MIT",
        ),
    });
    // Neutrals: near-black #0f0f0f titles, #2a2a2a body copy, #737373
    // metadata, #f0f0f0 selected rows and #e8e8e8 hairlines on a white
    // content panel beside the grey #efefef sidebar.
    zuse_code_roles(&mut palette);
    palette.colors.solid = c("#1a1a1a");
    palette.colors.on_solid = palette.colors.solid.best_on_color();
    palette.colors.text_strong = c("#0f0f0f");
    palette.colors.text_muted = c("#737373");
    palette.colors.selected = c("#f0f0f0");
    palette.colors.hover = c("#f4f4f4");
    palette.colors.border = c("#e8e8e8");
    palette.colors.border_strong = c("#dcdcdc");
    palette.colors.input = c("#e8e8e8");
    palette.colors.overlay = c("#ffffff");
    palette.colors.dialog = c("#ffffff");
    palette.accent.primary = c("#81bf1d");
    palette.accent.strong = c("#81bf1d");
    palette.accent.on = c("#0c170f");
    palette
}

fn glass(appearance: Appearance) -> ThemeVariant {
    variant(match appearance {
        Appearance::Light => Seeds {
            id: "glass-light",
            family_id: "glass",
            name: "Glass Light",
            appearance,
            treatment: SurfaceTreatment::Frosted,
            background: "#eaf0f8",
            shell: "#dce6f3",
            raised: "#e2ebf6",
            card: "#f6f9fe",
            text: "#25364c",
            muted: "#526078",
            faint: "#586681",
            accent: "#395e9c",
            danger: "#a42643",
            warning: "#87491e",
            success: "#1f6c59",
            terminal_background: "#eaf0f8",
            ansi: ANSI_LIGHT,
            syntax: [
                "#526078", "#65468d", "#246548", "#884a19", "#276577", "#31578e", "#734775",
                "#25364c", "#526078", "#a42643", "#734775", "#a42643",
            ],
            source: source(
                "glass-light",
                "native",
                "https://github.com/opcodehq/zuse",
                "appearance-glass",
                "MIT",
            ),
        },
        Appearance::Dark => Seeds {
            id: "glass-dark",
            family_id: "glass",
            name: "Glass Dark",
            appearance,
            treatment: SurfaceTreatment::Frosted,
            background: "#151e2d",
            shell: "#101724",
            raised: "#28374f",
            card: "#1b2638",
            text: "#e6eefb",
            muted: "#aabbd3",
            faint: "#8ca2c1",
            accent: "#a9c5ff",
            danger: "#ff9cab",
            warning: "#f0c28c",
            success: "#9bdac6",
            terminal_background: "#151e2d",
            ansi: ANSI_DARK,
            syntax: [
                "#aabbd3", "#ccb7f1", "#a4dac5", "#f0c28c", "#97d9e5", "#a9c5ff", "#e7b5d6",
                "#e6eefb", "#aabbd3", "#ff9cab", "#e7b5d6", "#ff9cab",
            ],
            source: source(
                "glass-dark",
                "native",
                "https://github.com/opcodehq/zuse",
                "appearance-glass",
                "MIT",
            ),
        },
    })
}

fn observatory_light() -> ThemeVariant {
    variant(Seeds {
        id: "observatory-light",
        family_id: "observatory",
        name: "Observatory Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#f4f7fb",
        shell: "#e5ecf3",
        raised: "#eaf0f7",
        card: "#ffffff",
        text: "#27364a",
        muted: "#566277",
        faint: "#627187",
        accent: "#8a531e",
        danger: "#a0263b",
        warning: "#8a531e",
        success: "#236548",
        terminal_background: "#f4f7fb",
        ansi: [
            "#e5ecf3", "#a0263b", "#236548", "#8a531e", "#235ba5", "#653a93", "#216a74", "#27364a",
            "#566277", "#a0263b", "#236548", "#8a531e", "#235ba5", "#735581", "#216a74", "#27364a",
        ],
        syntax: [
            "#566277", "#653a93", "#236548", "#8a531e", "#216a74", "#235ba5", "#7c4363", "#27364a",
            "#566277", "#a0263b", "#735581", "#a0263b",
        ],
        source: source(
            "observatory-light",
            "native",
            "https://github.com/opcodehq/zuse",
            "appearance-gallery",
            "MIT",
        ),
    })
}

fn observatory_dark() -> ThemeVariant {
    variant(Seeds {
        id: "observatory-dark",
        family_id: "observatory",
        name: "Observatory Dark",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#101823",
        shell: "#0c111a",
        raised: "#202d3e",
        card: "#192333",
        text: "#e6eef9",
        muted: "#a5b4c8",
        faint: "#8799af",
        accent: "#f3be73",
        danger: "#ff9aa6",
        warning: "#f3be73",
        success: "#92d8ae",
        terminal_background: "#101823",
        ansi: [
            "#0c111a", "#ff9aa6", "#92d8ae", "#f3be73", "#a5c9ff", "#ccb1f5", "#8fdbdb", "#e6eef9",
            "#202d3e", "#ff9aa6", "#92d8ae", "#f3be73", "#a5c9ff", "#ecadd5", "#8fdbdb", "#e6eef9",
        ],
        syntax: [
            "#a5b4c8", "#ccb1f5", "#92d8ae", "#f3be73", "#8fdbdb", "#a5c9ff", "#ecadd5", "#e6eef9",
            "#a5b4c8", "#ff9aa6", "#ecadd5", "#ff9aa6",
        ],
        source: source(
            "observatory-dark",
            "native",
            "https://github.com/opcodehq/zuse",
            "appearance-gallery",
            "MIT",
        ),
    })
}

fn flexoki_light() -> ThemeVariant {
    variant(Seeds {
        id: "flexoki-light",
        family_id: "flexoki",
        name: "Flexoki Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#fffcf0",
        shell: "#f2f0e5",
        raised: "#f2f0e5",
        card: "#fffcf0",
        text: "#100f0f",
        muted: "#6d6b66",
        faint: "#6f6e69",
        accent: "#bc5215",
        danger: "#af3029",
        warning: "#8e6b01",
        success: "#536907",
        terminal_background: "#fffcf0",
        ansi: [
            "#f2f0e5", "#af3029", "#536907", "#8e6b01", "#bc5215", "#a02f6f", "#1c6c66", "#100f0f",
            "#6f6e69", "#af3029", "#536907", "#5e409d", "#bc5215", "#a02f6f", "#1c6c66", "#100f0f",
        ],
        syntax: [
            "#6f6e69", "#a02f6f", "#536907", "#5e409d", "#1c6c66", "#bc5215", "#205ea6", "#100f0f",
            "#6f6e69", "#af3029", "#a02f6f", "#af3029",
        ],
        source: source(
            "flexoki-light",
            "palette",
            "https://github.com/kepano/flexoki",
            "8d723bac4a9ac46adfdf99d42155286977aac72a",
            "MIT",
        ),
    })
}

fn flexoki_dark() -> ThemeVariant {
    variant(Seeds {
        id: "flexoki-dark",
        family_id: "flexoki",
        name: "Flexoki Dark",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#100f0f",
        shell: "#1c1b1a",
        raised: "#282726",
        card: "#1c1b1a",
        text: "#cecdc3",
        muted: "#9f9d96",
        faint: "#878580",
        accent: "#da702c",
        danger: "#e8705f",
        warning: "#d0a215",
        success: "#a0af54",
        terminal_background: "#100f0f",
        ansi: [
            "#1c1b1a", "#e8705f", "#a0af54", "#d0a215", "#da702c", "#ce5d97", "#3aa99f", "#cecdc3",
            "#282726", "#e8705f", "#a0af54", "#8b7ec8", "#da702c", "#ce5d97", "#3aa99f", "#cecdc3",
        ],
        syntax: [
            "#9f9d96", "#ce5d97", "#a0af54", "#8b7ec8", "#3aa99f", "#da702c", "#66a0c8", "#cecdc3",
            "#9f9d96", "#e8705f", "#ce5d97", "#e8705f",
        ],
        source: source(
            "flexoki-dark",
            "palette",
            "https://github.com/kepano/flexoki",
            "8d723bac4a9ac46adfdf99d42155286977aac72a",
            "MIT",
        ),
    })
}

fn everforest_light() -> ThemeVariant {
    variant(Seeds {
        id: "everforest-light",
        family_id: "everforest",
        name: "Everforest Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#fdf6e3",
        shell: "#efebd4",
        raised: "#f4f0d9",
        card: "#f8f5e4",
        text: "#47555d",
        muted: "#5c6a72",
        faint: "#5c6a72",
        accent: "#627212",
        danger: "#a33738",
        warning: "#89541f",
        success: "#536d20",
        terminal_background: "#fdf6e3",
        ansi: [
            "#efebd4", "#a33738", "#536d20", "#89541f", "#386d79", "#954e62", "#356c5c", "#47555d",
            "#5c6a72", "#a33738", "#536d20", "#89541f", "#386d79", "#954e62", "#356c5c", "#47555d",
        ],
        syntax: [
            "#5c6a72", "#954e62", "#536d20", "#89541f", "#356c5c", "#386d79", "#75547e", "#47555d",
            "#5c6a72", "#a33738", "#954e62", "#a33738",
        ],
        source: source(
            "everforest-light",
            "palette",
            "https://github.com/sainnhe/everforest",
            "85a86eb62409e3ec88713bff3d1b9d7374e112e4",
            "MIT",
        ),
    })
}

fn everforest_dark() -> ThemeVariant {
    variant(Seeds {
        id: "everforest-dark",
        family_id: "everforest",
        name: "Everforest Dark",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#2d353b",
        shell: "#232a2e",
        raised: "#3b474b",
        card: "#343f44",
        text: "#e0d3b8",
        muted: "#abb7ae",
        faint: "#a6b2a9",
        accent: "#a7c080",
        danger: "#e67e80",
        warning: "#dbbc7f",
        success: "#a7c080",
        terminal_background: "#2d353b",
        ansi: [
            "#232a2e", "#e67e80", "#a7c080", "#dbbc7f", "#9ac7c8", "#d699b6", "#a0d3aa", "#e0d3b8",
            "#3b474b", "#e99587", "#a7c080", "#dbbc7f", "#9ac7c8", "#e99587", "#a0d3aa", "#e0d3b8",
        ],
        syntax: [
            "#a6b2a9", "#d699b6", "#a7c080", "#dbbc7f", "#a0d3aa", "#9ac7c8", "#d699b6", "#e0d3b8",
            "#a6b2a9", "#e99587", "#e99587", "#e67e80",
        ],
        source: source(
            "everforest-dark",
            "palette",
            "https://github.com/sainnhe/everforest",
            "85a86eb62409e3ec88713bff3d1b9d7374e112e4",
            "MIT",
        ),
    })
}

fn catppuccin_mocha() -> ThemeVariant {
    variant(Seeds {
        id: "catppuccin-mocha",
        family_id: "catppuccin",
        name: "Catppuccin Mocha",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#1e1e2e",
        shell: "#181825",
        raised: "#313244",
        card: "#242436",
        text: "#cdd6f4",
        muted: "#a6adc8",
        faint: "#9399b2",
        accent: "#cba6f7",
        danger: "#f38ba8",
        warning: "#f9e2af",
        success: "#a6e3a1",
        terminal_background: "#1e1e2e",
        ansi: [
            "#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de",
            "#585b70", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8",
        ],
        syntax: [
            "#6c7086", "#cba6f7", "#a6e3a1", "#fab387", "#f9e2af", "#89b4fa", "#94e2d5", "#cdd6f4",
            "#bac2de", "#f38ba8", "#f5c2e7", "#f38ba8",
        ],
        source: source(
            "catppuccin-mocha",
            "vscode",
            "https://github.com/catppuccin/vscode",
            "befc9e6fc41980f4241408f7049755d47c06ff45",
            "MIT",
        ),
    })
}

fn catppuccin_latte() -> ThemeVariant {
    variant(Seeds {
        id: "catppuccin-latte",
        family_id: "catppuccin",
        name: "Catppuccin Latte",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#eff1f5",
        shell: "#e6e9ef",
        raised: "#dce0e8",
        card: "#f7f8fa",
        text: "#4c4f69",
        muted: "#5c5f77",
        faint: "#6c6f85",
        accent: "#8839ef",
        danger: "#d20f39",
        warning: "#df8e1d",
        success: "#40a02b",
        terminal_background: "#eff1f5",
        ansi: [
            "#5c5f77", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#acb0be",
            "#6c6f85", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#bcc0cc",
        ],
        syntax: [
            "#8c8fa1", "#8839ef", "#40a02b", "#fe640b", "#df8e1d", "#1e66f5", "#179299", "#4c4f69",
            "#6c6f85", "#d20f39", "#ea76cb", "#d20f39",
        ],
        source: source(
            "catppuccin-latte",
            "vscode",
            "https://github.com/catppuccin/vscode",
            "befc9e6fc41980f4241408f7049755d47c06ff45",
            "MIT",
        ),
    })
}

fn tokyo_dark() -> ThemeVariant {
    variant(Seeds {
        id: "tokyo-night",
        family_id: "tokyo-night",
        name: "Tokyo Night",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#1a1b26",
        shell: "#16161e",
        raised: "#292e42",
        card: "#1f2335",
        text: "#c0caf5",
        muted: "#9aa5ce",
        faint: "#7f87ad",
        accent: "#7aa2f7",
        danger: "#f7768e",
        warning: "#e0af68",
        success: "#9ece6a",
        terminal_background: "#1a1b26",
        ansi: [
            "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
            "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5",
        ],
        syntax: [
            "#565f89", "#bb9af7", "#9ece6a", "#ff9e64", "#2ac3de", "#7aa2f7", "#73daca", "#c0caf5",
            "#9abdf5", "#f7768e", "#e0af68", "#f7768e",
        ],
        source: source(
            "tokyo-night",
            "vscode",
            "https://github.com/tokyo-night/tokyo-night-vscode-theme",
            "7c0f11eaef322f293621ca7befe462214b7ea468",
            "MIT",
        ),
    })
}

fn tokyo_light() -> ThemeVariant {
    variant(Seeds {
        id: "tokyo-night-light",
        family_id: "tokyo-night",
        name: "Tokyo Night Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#d5d6db",
        shell: "#cbccd1",
        raised: "#bfc0c6",
        card: "#e1e2e7",
        text: "#343b59",
        muted: "#485e7d",
        faint: "#5a6378",
        accent: "#34548a",
        danger: "#8c4351",
        warning: "#8f5e15",
        success: "#33635c",
        terminal_background: "#d5d6db",
        ansi: [
            "#0f0f14", "#8c4351", "#33635c", "#8f5e15", "#34548a", "#5a4a78", "#0f4b6e", "#828594",
            "#4c505e", "#8c4351", "#33635c", "#8f5e15", "#34548a", "#5a4a78", "#0f4b6e", "#343b59",
        ],
        syntax: [
            "#6c6e75", "#5a4a78", "#33635c", "#965027", "#0f4b6e", "#34548a", "#166775", "#343b59",
            "#485e7d", "#8c4351", "#8f5e15", "#8c4351",
        ],
        source: source(
            "tokyo-night-light",
            "vscode",
            "https://github.com/tokyo-night/tokyo-night-vscode-theme",
            "7c0f11eaef322f293621ca7befe462214b7ea468",
            "MIT",
        ),
    })
}

fn dracula() -> ThemeVariant {
    variant(Seeds {
        id: "dracula",
        family_id: "dracula",
        name: "Dracula",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#282a36",
        shell: "#21222c",
        raised: "#44475a",
        card: "#2d303e",
        text: "#f8f8f2",
        muted: "#c5c8c6",
        faint: "#9a9cab",
        accent: "#bd93f9",
        danger: "#ff5555",
        warning: "#f1fa8c",
        success: "#50fa7b",
        terminal_background: "#282a36",
        ansi: [
            "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2",
            "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff",
        ],
        syntax: [
            "#6272a4", "#ff79c6", "#f1fa8c", "#bd93f9", "#8be9fd", "#50fa7b", "#8be9fd", "#f8f8f2",
            "#f8f8f2", "#ff79c6", "#50fa7b", "#ff5555",
        ],
        source: source(
            "dracula",
            "vscode",
            "https://github.com/dracula/visual-studio-code",
            "1b9ecf4d7e0c8cc2e2e890a7a41ad1db5fff1e6c",
            "MIT",
        ),
    })
}

fn github_light() -> ThemeVariant {
    variant(Seeds {
        id: "github-light",
        family_id: "github",
        name: "GitHub Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#ffffff",
        shell: "#f6f8fa",
        raised: "#d0d7de",
        card: "#ffffff",
        text: "#1f2328",
        muted: "#656d76",
        faint: "#818b98",
        accent: "#0969da",
        danger: "#cf222e",
        warning: "#9a6700",
        success: "#1a7f37",
        terminal_background: "#ffffff",
        ansi: [
            "#24292f", "#cf222e", "#1a7f37", "#9a6700", "#0969da", "#8250df", "#1b7c83", "#6e7781",
            "#57606a", "#a40e26", "#116329", "#7d4e00", "#0550ae", "#6639ba", "#0a6b74", "#1f2328",
        ],
        syntax: [
            "#6e7781", "#cf222e", "#0a3069", "#0550ae", "#953800", "#8250df", "#116329", "#1f2328",
            "#57606a", "#116329", "#953800", "#cf222e",
        ],
        source: source(
            "github-light",
            "vscode",
            "https://github.com/primer/github-vscode-theme",
            "cd78e5e4e7bcf132a6f428ae0f32264bb1b729cf",
            "MIT",
        ),
    })
}

fn github_dark() -> ThemeVariant {
    variant(Seeds {
        id: "github-dark",
        family_id: "github",
        name: "GitHub Dark",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#0d1117",
        shell: "#010409",
        raised: "#21262d",
        card: "#161b22",
        text: "#e6edf3",
        muted: "#8b949e",
        faint: "#6e7681",
        accent: "#2f81f7",
        danger: "#f85149",
        warning: "#d29922",
        success: "#3fb950",
        terminal_background: "#0d1117",
        ansi: [
            "#484f58", "#ff7b72", "#3fb950", "#d29922", "#58a6ff", "#bc8cff", "#39c5cf", "#b1bac4",
            "#6e7681", "#ffa198", "#56d364", "#e3b341", "#79c0ff", "#d2a8ff", "#56d4dd", "#f0f6fc",
        ],
        syntax: [
            "#8b949e", "#ff7b72", "#a5d6ff", "#79c0ff", "#ffa657", "#d2a8ff", "#7ee787", "#e6edf3",
            "#b1bac4", "#7ee787", "#ffa657", "#f85149",
        ],
        source: source(
            "github-dark",
            "vscode",
            "https://github.com/primer/github-vscode-theme",
            "cd78e5e4e7bcf132a6f428ae0f32264bb1b729cf",
            "MIT",
        ),
    })
}

fn ayu_light() -> ThemeVariant {
    variant(Seeds {
        id: "ayu-light",
        family_id: "ayu",
        name: "Ayu Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#fcfcfc",
        shell: "#f8f9fa",
        raised: "#e6e9ed",
        card: "#fafafa",
        text: "#5c6166",
        muted: "#828e9f",
        faint: "#969da8",
        accent: "#f29718",
        danger: "#e65050",
        warning: "#eba400",
        success: "#6cbf43",
        terminal_background: "#f8f9fa",
        ansi: [
            "#000000", "#f06b6c", "#6cbf43", "#e7a100", "#21a1e2", "#a176cb", "#4abc96", "#c7c7c7",
            "#686868", "#f07171", "#86b300", "#eba400", "#22a4e6", "#a37acc", "#4cbf99", "#d1d1d1",
        ],
        syntax: [
            "#adaeb1", "#fa8532", "#86b300", "#a37acc", "#55b4d4", "#f29718", "#f07171", "#5c6166",
            "#828e9f", "#55b4d4", "#e59645", "#e65050",
        ],
        source: source(
            "ayu-light",
            "vscode",
            "https://github.com/ayu-theme/vscode-ayu",
            "444ef92911cb75c3933c8003e3a7c79b6b6c914f",
            "MIT",
        ),
    })
}

fn ayu_dark() -> ThemeVariant {
    variant(Seeds {
        id: "ayu-dark",
        family_id: "ayu",
        name: "Ayu Dark",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#10141c",
        shell: "#0d1017",
        raised: "#1b1f29",
        card: "#141821",
        text: "#bfbdb6",
        muted: "#8a9199",
        faint: "#5a6673",
        accent: "#e6b450",
        danger: "#d95757",
        warning: "#ffb454",
        success: "#70bf56",
        terminal_background: "#0d1017",
        ansi: [
            "#1b1f29", "#f06b73", "#70bf56", "#fdb04c", "#4fbfff", "#d0a1ff", "#93e2c8", "#c7c7c7",
            "#686868", "#f07178", "#aad94c", "#ffb454", "#59c2ff", "#d2a6ff", "#95e6cb", "#ffffff",
        ],
        syntax: [
            "#5a6673", "#ff8f40", "#aad94c", "#d2a6ff", "#39bae6", "#ffb454", "#f07178", "#bfbdb6",
            "#8a9199", "#39bae6", "#e6c08a", "#d95757",
        ],
        source: source(
            "ayu-dark",
            "vscode",
            "https://github.com/ayu-theme/vscode-ayu",
            "444ef92911cb75c3933c8003e3a7c79b6b6c914f",
            "MIT",
        ),
    })
}

fn gruvbox_light() -> ThemeVariant {
    variant(Seeds {
        id: "gruvbox-light",
        family_id: "gruvbox",
        name: "Gruvbox Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#fbf1c7",
        shell: "#f2e5bc",
        raised: "#d5c4a1",
        card: "#f9f5d7",
        text: "#3c3836",
        muted: "#665c54",
        faint: "#7c6f64",
        accent: "#076678",
        danger: "#9d0006",
        warning: "#b57614",
        success: "#79740e",
        terminal_background: "#fbf1c7",
        ansi: [
            "#ebdbb2", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#7c6f64",
            "#928374", "#9d0006", "#79740e", "#b57614", "#076678", "#8f3f71", "#427b58", "#3c3836",
        ],
        syntax: [
            "#928374", "#9d0006", "#79740e", "#8f3f71", "#b57614", "#076678", "#427b58", "#3c3836",
            "#665c54", "#9d0006", "#af3a03", "#cc241d",
        ],
        source: source(
            "gruvbox-light",
            "vscode",
            "https://github.com/jdinhify/vscode-theme-gruvbox",
            "ca3b8ad203e84a884ca33fb84b5795cf43032709",
            "MIT",
        ),
    })
}

fn gruvbox_dark() -> ThemeVariant {
    variant(Seeds {
        id: "gruvbox-dark",
        family_id: "gruvbox",
        name: "Gruvbox Dark",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#282828",
        shell: "#1d2021",
        raised: "#504945",
        card: "#32302f",
        text: "#ebdbb2",
        muted: "#bdae93",
        faint: "#928374",
        accent: "#8ec07c",
        danger: "#fb4934",
        warning: "#fabd2f",
        success: "#b8bb26",
        terminal_background: "#282828",
        ansi: [
            "#3c3836", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984",
            "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2",
        ],
        syntax: [
            "#928374", "#fb4934", "#b8bb26", "#d3869b", "#fabd2f", "#83a598", "#8ec07c", "#ebdbb2",
            "#bdae93", "#fb4934", "#fe8019", "#fb4934",
        ],
        source: source(
            "gruvbox-dark",
            "vscode",
            "https://github.com/jdinhify/vscode-theme-gruvbox",
            "ca3b8ad203e84a884ca33fb84b5795cf43032709",
            "MIT",
        ),
    })
}

fn rose_pine_dawn() -> ThemeVariant {
    variant(Seeds {
        id: "rose-pine-dawn",
        family_id: "rose-pine",
        name: "Rosé Pine Dawn",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#faf4ed",
        shell: "#f2e9e1",
        raised: "#dfdad9",
        card: "#fffaf3",
        text: "#575279",
        muted: "#5f5a78",
        faint: "#9893a5",
        accent: "#907aa9",
        danger: "#b4637a",
        warning: "#ea9d34",
        success: "#56949f",
        terminal_background: "#faf4ed",
        ansi: [
            "#f2e9e1", "#b4637a", "#286983", "#ea9d34", "#56949f", "#907aa9", "#d7827e", "#575279",
            "#797593", "#b4637a", "#286983", "#ea9d34", "#56949f", "#907aa9", "#d7827e", "#575279",
        ],
        syntax: [
            "#9893a5", "#286983", "#ea9d34", "#d7827e", "#56949f", "#b4637a", "#56949f", "#d7827e",
            "#797593", "#56949f", "#907aa9", "#b4637a",
        ],
        source: source(
            "rose-pine-dawn",
            "vscode",
            "https://github.com/rose-pine/vscode",
            "d8f5ebe8e096fa833e997c07eb7685ee1677a4ba",
            "MIT",
        ),
    })
}

fn rose_pine_moon() -> ThemeVariant {
    variant(Seeds {
        id: "rose-pine-moon",
        family_id: "rose-pine",
        name: "Rosé Pine Moon",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#232136",
        shell: "#1f1d2e",
        raised: "#393552",
        card: "#2a273f",
        text: "#e0def4",
        muted: "#aaa6c1",
        faint: "#6e6a86",
        accent: "#c4a7e7",
        danger: "#eb6f92",
        warning: "#f6c177",
        success: "#9ccfd8",
        terminal_background: "#232136",
        ansi: [
            "#393552", "#eb6f92", "#3e8fb0", "#f6c177", "#9ccfd8", "#c4a7e7", "#ea9a97", "#e0def4",
            "#908caa", "#eb6f92", "#3e8fb0", "#f6c177", "#9ccfd8", "#c4a7e7", "#ea9a97", "#e0def4",
        ],
        syntax: [
            "#6e6a86", "#3e8fb0", "#f6c177", "#ea9a97", "#9ccfd8", "#eb6f92", "#9ccfd8", "#ea9a97",
            "#908caa", "#9ccfd8", "#c4a7e7", "#eb6f92",
        ],
        source: source(
            "rose-pine-moon",
            "vscode",
            "https://github.com/rose-pine/vscode",
            "d8f5ebe8e096fa833e997c07eb7685ee1677a4ba",
            "MIT",
        ),
    })
}

fn nord() -> ThemeVariant {
    variant(Seeds {
        id: "nord",
        family_id: "nord",
        name: "Nord",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#2e3440",
        shell: "#292e39",
        raised: "#434c5e",
        card: "#3b4252",
        text: "#d8dee9",
        muted: "#aeb6c4",
        faint: "#7f899b",
        accent: "#88c0d0",
        danger: "#bf616a",
        warning: "#ebcb8b",
        success: "#a3be8c",
        terminal_background: "#2e3440",
        ansi: [
            "#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0",
            "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4",
        ],
        syntax: [
            "#616e88", "#81a1c1", "#a3be8c", "#b48ead", "#8fbcbb", "#88c0d0", "#d08770", "#d8dee9",
            "#aeb6c4", "#81a1c1", "#8fbcbb", "#bf616a",
        ],
        source: source(
            "nord",
            "vscode",
            "https://github.com/nordtheme/visual-studio-code",
            "8ead09822c02d0d49d0f764104505e5a34d3689f",
            "MIT",
        ),
    })
}

fn one_dark_pro() -> ThemeVariant {
    variant(Seeds {
        id: "one-dark-pro",
        family_id: "one-dark-pro",
        name: "One Dark Pro",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#282c34",
        shell: "#21252b",
        raised: "#2c313a",
        card: "#21252b",
        text: "#abb2bf",
        muted: "#9da5b4",
        faint: "#7f848e",
        accent: "#528bff",
        danger: "#e05561",
        warning: "#d19a66",
        success: "#8cc265",
        terminal_background: "#282c34",
        ansi: [
            "#3f4451", "#e05561", "#8cc265", "#d18f52", "#4aa5f0", "#c162de", "#42b3c2", "#d7dae0",
            "#4f5666", "#ff616e", "#a5e075", "#f0a45d", "#4dc4ff", "#de73ff", "#4cd1e0", "#e6e6e6",
        ],
        syntax: [
            "#7f848e", "#c678dd", "#e06c75", "#c678dd", "#e5c07b", "#e5c07b", "#d19a66", "#abb2bf",
            "#abb2bf", "#c678dd", "#56b6c2", "#e05561",
        ],
        source: source(
            "one-dark-pro",
            "vscode",
            "https://github.com/Binaryify/OneDark-Pro",
            "e6ccf638d5b69aa38cd1005edb0ee7ba7ef6fedc",
            "MIT",
        ),
    })
}
