//! Runtime backend selection + session-aware construction. The one place that
//! knows every backend crate: maps a [`Backend`] to its [`BackendFactory`], then
//! derives the session id, loads/creates the record, opens (resume or spawn) the
//! controller, and persists. Adding a backend = one new match arm + its crate.

use agent_controller_android_emu::AndroidEmuFactory;
use agent_controller_chrome::ChromeFactory;
use agent_controller_core::{
    anyhow, Backend, BackendFactory, Controller, Options, Result, SessionRecord, SessionStore,
};
use agent_controller_firefox::FirefoxFactory;
#[cfg(target_os = "macos")]
use agent_controller_ios_sim::IosSimFactory;
#[cfg(target_os = "macos")]
use agent_controller_mac::MacFactory;
#[cfg(target_os = "macos")]
use agent_controller_safari::SafariFactory;
#[cfg(target_os = "windows")]
use agent_controller_windows::WindowsFactory;

/// The backend factory for `backend` on this platform.
pub fn registry(backend: Backend) -> Result<Box<dyn BackendFactory>> {
    match backend {
        Backend::Firefox => Ok(Box::new(FirefoxFactory)),
        Backend::Chrome => Ok(Box::new(ChromeFactory)),
        Backend::AndroidEmu => Ok(Box::new(AndroidEmuFactory)),
        #[cfg(target_os = "macos")]
        Backend::Mac => Ok(Box::new(MacFactory)),
        #[cfg(target_os = "macos")]
        Backend::IosSim => Ok(Box::new(IosSimFactory)),
        #[cfg(target_os = "macos")]
        Backend::Safari => Ok(Box::new(SafariFactory)),
        #[cfg(target_os = "windows")]
        Backend::Windows => Ok(Box::new(WindowsFactory)),
        #[allow(unreachable_patterns)]
        other => Err(anyhow!(
            "backend `{}` is not available on this platform",
            other.as_str()
        )),
    }
}

/// Open (resume or cold-start) the session for `backend`+`opts`, persisting the
/// record. Returns the controller and the resolved session id.
pub async fn create(
    store: &SessionStore,
    backend: Backend,
    opts: Options,
) -> Result<(Box<dyn Controller>, String)> {
    let factory = registry(backend)?;
    let ident = factory.identify(&opts).await?;
    let mut rec = store
        .load(&ident.id)?
        .unwrap_or_else(|| SessionRecord::new(&ident.id, backend, &ident.target));
    rec.target = ident.target.clone();
    rec.touch();
    let ctrl = factory.open(&mut rec, store, &opts).await?;
    store.save(&rec)?;
    Ok((ctrl, ident.id))
}
