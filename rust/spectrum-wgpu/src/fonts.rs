//! The system's own interface fonts, as the web page asks for them (web/style.css `--sans`,
//! `--mono`): the platform's UI face for text, its Chinese face for Chinese, its monospace
//! for figures. Nothing is bundled; egui's built-in fonts stay last as a fallback.
//! `SPECTRUM_FONT=/path/to/font` puts one file of the user's choosing in front of all.
use std::{path::PathBuf, sync::Arc};

/// Text sizes of web/style.css: `--text-base` and `--text-xs`.
pub const BASE: f32 = 12.0;
pub const SMALL: f32 = 10.0;

/// A font file and the face inside it (a .ttc holds several).
struct Face {
    path: PathBuf,
    index: u32,
    /// Moves the glyphs down by this fraction of the size, for a face whose vertical
    /// metrics put it off the system face's baseline.
    lower: f32,
}
fn face(path: impl Into<PathBuf>, index: u32) -> Face {
    Face {
        path: path.into(),
        index,
        lower: 0.0,
    }
}

/// Hiragino Sans GB sits above San Francisco's baseline in egui; measured on screenshots.
#[cfg(target_os = "macos")]
const HIRAGINO_LOWER: f32 = 0.18;

/// Candidates in order of preference: (text, Chinese, monospace).
#[cfg(target_os = "macos")]
fn candidates() -> (Vec<Face>, Vec<Face>, Vec<Face>) {
    let fonts = PathBuf::from("/System/Library/Fonts");
    // PingFang ships as a downloaded system asset whose folder name is a hash.
    let pingfang = std::fs::read_dir("/System/Library/AssetsV2")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("com_apple_MobileAsset_Font")
        })
        .flat_map(|e| std::fs::read_dir(e.path()).into_iter().flatten().flatten())
        .map(|e| e.path().join("AssetData/PingFang.ttc"))
        .filter(|p| p.is_file())
        // PingFang SC Regular is the fourth face (HK, MO, TC, SC).
        .map(|p| face(p, 3));
    (
        vec![
            face(fonts.join("SFNS.ttf"), 0),
            face(fonts.join("Helvetica.ttc"), 0),
        ],
        // Hiragino Sans GB (W3) first: it is close to PingFang and costs about 15 MB of
        // memory where PingFang's 78 MB collection costs about 85 MB.
        std::iter::once(Face {
            lower: HIRAGINO_LOWER,
            ..face(fonts.join("Hiragino Sans GB.ttc"), 0)
        })
        .chain(pingfang)
        .chain([face(fonts.join("STHeiti Light.ttc"), 0)])
        .collect(),
        vec![
            face(fonts.join("SFNSMono.ttf"), 0),
            face(fonts.join("Menlo.ttc"), 0),
        ],
    )
}

#[cfg(target_os = "windows")]
fn candidates() -> (Vec<Face>, Vec<Face>, Vec<Face>) {
    let fonts = PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()))
        .join("Fonts");
    (
        vec![face(fonts.join("segoeui.ttf"), 0)],
        vec![
            face(fonts.join("msyh.ttc"), 0),
            face(fonts.join("simsun.ttc"), 0),
        ],
        vec![
            face(fonts.join("consola.ttf"), 0),
            face(fonts.join("cour.ttf"), 0),
        ],
    )
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn candidates() -> (Vec<Face>, Vec<Face>, Vec<Face>) {
    // fontconfig knows the desktop's choice; the fixed paths cover systems without it.
    let ask = |pattern: &str| -> Option<Face> {
        let out = std::process::Command::new("fc-match")
            .args(["-f", "%{file}\n%{index}", pattern])
            .output()
            .ok()?;
        let text = String::from_utf8(out.stdout).ok()?;
        let mut lines = text.lines();
        let path = lines.next().filter(|p| !p.is_empty())?;
        Some(face(
            path,
            lines.next().and_then(|i| i.parse().ok()).unwrap_or(0),
        ))
    };
    let fixed = |paths: &[(&str, u32)]| paths.iter().map(|(p, i)| face(*p, *i)).collect::<Vec<_>>();
    let mut sans: Vec<_> = ask("sans-serif").into_iter().collect();
    sans.extend(fixed(&[
        ("/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", 0),
        ("/usr/share/fonts/noto/NotoSans-Regular.ttf", 0),
        ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 0),
        ("/usr/share/fonts/TTF/DejaVuSans.ttf", 0),
    ]));
    let mut cjk: Vec<_> = ask("sans-serif:lang=zh-cn").into_iter().collect();
    // In Noto Sans CJK collections the faces run JP, KR, SC, TC, HK.
    cjk.extend(fixed(&[
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 2),
        ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 2),
        (
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            2,
        ),
        ("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc", 0),
        (
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
            0,
        ),
    ]));
    let mut mono: Vec<_> = ask("monospace").into_iter().collect();
    mono.extend(fixed(&[
        ("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", 0),
        ("/usr/share/fonts/TTF/DejaVuSansMono.ttf", 0),
        ("/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf", 0),
    ]));
    (sans, cjk, mono)
}

/// The first candidate that reads, under `name` in `fonts`.
fn load(fonts: &mut egui::FontDefinitions, name: &str, list: Vec<Face>) -> Option<String> {
    for f in list {
        if let Ok(bytes) = std::fs::read(&f.path) {
            // Kept for the life of the program anyway. egui copies owned font data once
            // more for the rasteriser and borrows static data, so this holds one copy.
            let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
            let mut data = egui::FontData::from_static(bytes);
            data.index = f.index;
            data.tweak.y_offset_factor = f.lower;
            fonts.font_data.insert(name.into(), Arc::new(data));
            return Some(name.into());
        }
    }
    None
}

/// Installs the fonts and the web page's text sizes and tighter spacing.
pub fn install(ctx: &egui::Context) {
    let (sans, cjk, mono) = candidates();
    let mut fonts = egui::FontDefinitions::default();
    let custom = std::env::var_os("SPECTRUM_FONT")
        .and_then(|p| load(&mut fonts, "custom", vec![face(p, 0)]));
    let sans = load(&mut fonts, "system-sans", sans);
    let cjk = load(&mut fonts, "system-cjk", cjk);
    let mono = load(&mut fonts, "system-mono", mono);
    // Each family: the user's font, the system face, the Chinese face, then egui's own.
    for (family, primary) in [
        (egui::FontFamily::Proportional, &sans),
        (egui::FontFamily::Monospace, &mono),
    ] {
        let list = fonts.families.entry(family).or_default();
        let front: Vec<String> = [&custom, primary, &cjk]
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        list.retain(|n| !front.contains(n));
        list.splice(0..0, front);
    }
    ctx.set_fonts(fonts);
    ctx.all_styles_mut(|style| {
        use egui::{FontFamily::*, FontId, TextStyle};
        style.text_styles = [
            (TextStyle::Heading, FontId::new(BASE, Proportional)),
            (TextStyle::Body, FontId::new(BASE, Proportional)),
            (TextStyle::Button, FontId::new(BASE, Proportional)),
            (TextStyle::Monospace, FontId::new(BASE, Monospace)),
            (TextStyle::Small, FontId::new(SMALL, Proportional)),
        ]
        .into();
        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(8.0, 4.0);
        s.button_padding = egui::vec2(6.0, 2.0);
        s.interact_size.y = 20.0;
        s.icon_width = 12.0;
        s.icon_width_inner = 6.0;
        s.icon_spacing = 6.0;
    });
}
