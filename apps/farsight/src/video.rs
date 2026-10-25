//! Open H.264 for the viewer (M10): Cisco's OpenH264, downloaded on request
//! (U-12), used only with `-H264` and only in builds with the `h264` feature
//! (off in releases until tested by hand, U-14).

use farsight_codec::video::SharedCodecs;

#[cfg(feature = "h264")]
fn data_dir() -> Option<std::path::PathBuf> {
    farsight_vault::paths::data_home().map(|d| d.join("farsight"))
}

/// `--install-openh264`: downloads Cisco's library. Returns a message.
pub fn install() -> Result<String, String> {
    #[cfg(feature = "h264")]
    {
        let dir = data_dir().ok_or("no data directory")?;
        let path = farsight_h264::install(&dir).map_err(|e| e.to_string())?;
        Ok(format!(
            "Installed {}.\n{}",
            path.display(),
            farsight_h264::NOTICE
        ))
    }
    #[cfg(not(feature = "h264"))]
    Err("this build has no H.264 support (it needs the `h264` feature)".into())
}

/// The codecs for `-H264`; `None` without it or when unavailable (the viewer
/// still works, without H.264).
pub fn codecs(wanted: bool) -> Option<SharedCodecs> {
    if !wanted {
        return None;
    }
    #[cfg(feature = "h264")]
    {
        match data_dir().map(|dir| farsight_h264::OpenH264::load(&dir)) {
            Some(Ok(codecs)) => {
                tracing::info!("{}", farsight_h264::NOTICE);
                Some(SharedCodecs(std::sync::Arc::new(codecs)))
            }
            Some(Err(error)) => {
                tracing::warn!(%error, "H.264 isn't available; run `farsight --install-openh264`");
                None
            }
            None => None,
        }
    }
    #[cfg(not(feature = "h264"))]
    {
        tracing::warn!("this build has no H.264 support");
        None
    }
}
