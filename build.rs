use std::str::FromStr;

use {
  std::{env, io, path},
  winres::WindowsResource,
};

fn out_dir() -> path::PathBuf {
  path::PathBuf::from_str(&env::var("OUT_DIR").expect("OUT_DIR env var")).expect("out_dir path")
}

fn main() -> io::Result<()> {
  build_app_icon()?;
  build_font_definitions()?;
  Ok(())
}

fn build_app_icon() -> io::Result<()> {
  if env::var_os("CARGO_CFG_WINDOWS").is_some() {
    WindowsResource::new().set_icon("assets/icon.ico").compile()?;
  }

  println!("cargo:rerun-if-changed=assets/icon.ico");
  let file = std::fs::read("assets/icon.ico")?;

  let icon_dir = ico::IconDir::read(std::io::Cursor::new(file)).unwrap();
  let image = icon_dir.entries()[0].decode().unwrap();
  let mut rgba = image.rgba_data().to_vec();
  assert!(image.width() <= 256);
  assert!(image.height() <= 256);

  rgba.push((image.width() - 1) as u8);
  rgba.push((image.height() - 1) as u8);

  let compressed = lz4_flex::compress_prepend_size(&rgba);
  std::fs::write(out_dir().join("icon-rgba.lz4"), compressed)?;

  Ok(())
}

fn build_font_definitions() -> io::Result<()> {
  let fd = load_font_definitions()?;

  let mut bytes = Vec::new();
  ciborium::ser::into_writer(&fd, &mut bytes).unwrap();

  let bytes = lz4_flex::compress_prepend_size(&bytes);

  std::fs::write(out_dir().join("fonts.cib.lz4"), bytes)?;
  Ok(())
}

fn load_font_definitions() -> io::Result<egui::FontDefinitions> {
  let mut font_definitions = egui::FontDefinitions::empty();

  let fonts = [
    ("firacode-medium", "assets/fonts/firacode/FiraCodeNerdFont-Medium.ttf", None),
    ("terminus", "assets/fonts/terminus/TerminessNerdFont-Regular.ttf", None),
    // egui default two icon fonts
    (
      "noto-emoji",
      "assets/fonts/noto-emoji/NotoEmoji-VariableFont_wght.ttf",
      Some(egui::FontTweak {
        scale: 0.81, // make it smaller
        ..Default::default()
      }),
    ),
    (
      "emoji-icon-font",
      "assets/fonts/emoji/emoji.ttf",
      Some(egui::FontTweak {
        scale: 0.88, // make it smaller
        // probably not correct, but this does make texts look better (#2724 for details)
        y_offset_factor: 0.11,         // move glyphs down to better align with common fonts
        baseline_offset_factor: -0.11, // ...now the entire row is a bit down so shift it back
        ..Default::default()
      }),
    ),
  ];

  for font in fonts {
    println!("cargo:rerun-if-changed={}", font.1);
    let data = std::fs::read(font.1)?;
    let mut font_data = egui::FontData::from_owned(data);
    if let Some(tweak) = font.2 {
      font_data = font_data.tweak(tweak);
    }
    font_definitions.font_data.insert(font.0.into(), font_data);
  }

  let font_list: Vec<_> = ["firacode-medium", "noto-emoji", "emoji-icon-font"]
    .into_iter()
    .map(String::from)
    .collect();

  font_definitions.families.insert(egui::FontFamily::Monospace, font_list.clone());
  font_definitions.families.insert(egui::FontFamily::Proportional, font_list);

  Ok(font_definitions)
}
