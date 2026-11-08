//! Ressources embarquées (icônes SVG) pour GPUI.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(match path {
            "icons/camera.svg" => Some(Cow::Borrowed(include_bytes!("../assets/icons/camera.svg"))),
            _ => None,
        })
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(vec!["icons/camera.svg".into()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_asset_is_embedded() {
        let assets = Assets;
        let bytes = assets.load("icons/camera.svg").unwrap().unwrap();
        assert!(bytes.starts_with(b"<svg"));
        assert!(assets.load("icons/missing.svg").unwrap().is_none());
    }
}
