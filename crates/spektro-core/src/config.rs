//! User configuration: roots, templates, outputs, preset.

use crate::template::Template;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Templates {
    /// RAW files.
    pub raw: Template,
    /// JPEG/HEIF shot alongside a RAW. Default: next to the RAW.
    pub paired_image: Template,
    /// Images without a RAW.
    pub image: Template,
    pub video: Template,
    /// Sidecars and unrecognised files. Attachments of a RAW/video follow their primary instead.
    pub other: Template,
    pub render_exr: Template,
    pub render_jpeg: Template,
}

impl Default for Templates {
    fn default() -> Self {
        let t = |s: &str| Template::parse(s).expect("default template");
        Templates {
            raw: t("{year}/{date}/{camera}/{stem}.{ext}"),
            paired_image: t("{year}/{date}/{camera}/{stem}.{ext}"),
            image: t("{year}/{date}/{camera}/{stem}.{ext}"),
            video: t("{year}/{date}/video/{stem}.{ext}"),
            other: t("{import_date}/other/{rel_dir}/{stem}.{ext}"),
            render_exr: t("{year}/{date}/{camera}/{stem}.exr"),
            render_jpeg: t("{year}/{date}/{camera}/{stem}.jpg"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verify {
    /// Re-read the destination and compare BLAKE3 hashes.
    Hash,
    /// Trust the copy if sizes match.
    SizeOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Outputs {
    pub exr: bool,
    pub jpeg: bool,
    /// spektrafilm output colour space name for EXR: `Rec. 2020` or `ACES2065-1`.
    pub exr_color_space: String,
    pub jpeg_quality: u8,
}

impl Default for Outputs {
    fn default() -> Self {
        Outputs { exr: false, jpeg: true, exr_color_space: "Rec. 2020".into(), jpeg_quality: 92 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Where RAWs and images go.
    pub archive_root: PathBuf,
    /// Where videos go. Defaults to `archive_root`.
    pub video_root: Option<PathBuf>,
    /// Where sidecars/unknown files go. Defaults to `archive_root`.
    pub other_root: Option<PathBuf>,
    /// Where rendered EXR/JPEG go.
    pub render_root: PathBuf,
    pub templates: Templates,
    pub outputs: Outputs,
    /// Preset file (JSON) for the film stage.
    pub preset: PathBuf,
    /// spektrafilm data directory. `None` = next to the executable / vendored default.
    pub data_dir: Option<PathBuf>,
    pub verify: Verify,
    /// Skip a file if the destination already exists with the same size.
    pub skip_existing: bool,
    /// Number of parallel RAW decodes feeding the single GPU render thread.
    pub decode_threads: usize,
}

impl Default for Config {
    fn default() -> Self {
        let home = dirs_home();
        Config {
            archive_root: home.join("Pictures/Archive"),
            video_root: None,
            other_root: None,
            render_root: home.join("Pictures/Renders"),
            templates: Templates::default(),
            outputs: Outputs::default(),
            // A bare file name is looked up in the preset folders (see film::resolve_preset).
            preset: PathBuf::from("default.json"),
            data_dir: None,
            verify: Verify::Hash,
            skip_existing: true,
            decode_threads: std::thread::available_parallelism().map(|n| (n.get() / 2).clamp(2, 4)).unwrap_or(2),
        }
    }
}

impl Config {
    pub fn video_root(&self) -> &Path {
        self.video_root.as_deref().unwrap_or(&self.archive_root)
    }
    pub fn other_root(&self) -> &Path {
        self.other_root.as_deref().unwrap_or(&self.archive_root)
    }

    pub fn load(path: &Path) -> anyhow::Result<Config> {
        let text = std::fs::read_to_string(path)?;
        let mut cfg: Config = toml::from_str(&text)?;
        cfg.expand_home();
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    fn expand_home(&mut self) {
        let home = dirs_home();
        let fix = |p: &mut PathBuf| {
            if let Ok(rest) = p.strip_prefix("~") {
                *p = home.join(rest);
            }
        };
        fix(&mut self.archive_root);
        fix(&mut self.render_root);
        fix(&mut self.preset);
        if let Some(p) = &mut self.video_root {
            fix(p);
        }
        if let Some(p) = &mut self.other_root {
            fix(p);
        }
        if let Some(p) = &mut self.data_dir {
            fix(p);
        }
    }
}

pub fn dirs_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_roundtrip() {
        let cfg = Config::default();
        let s = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn partial_toml_uses_defaults() {
        let cfg: Config = toml::from_str("archive_root = \"/tmp/a\"\n[outputs]\nexr = true\n").unwrap();
        assert_eq!(cfg.archive_root, PathBuf::from("/tmp/a"));
        assert!(cfg.outputs.exr);
        assert!(cfg.outputs.jpeg);
        assert_eq!(cfg.templates, Templates::default());
    }
}
