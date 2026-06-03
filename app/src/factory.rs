//! Runtime backend selection + session-aware construction. The one place that
//! knows every backend crate: maps a [`Backend`] to its [`BackendFactory`], then
//! derives the session id, loads/creates the record, opens (resume or spawn) the
//! controller, and persists. Adding a backend = one new match arm + its crate.

use agent_controller_chrome::ChromeFactory;
use agent_controller_core::{
    Backend, BackendFactory, Controller, Options, Result, SessionRecord, SessionStore,
};
use agent_controller_firefox::FirefoxFactory;
use agent_controller_ios_sim::IosSimFactory;
use agent_controller_mac::MacFactory;
use agent_controller_safari::SafariFactory;

/// The backend factory for `backend`.
pub fn registry(backend: Backend) -> Result<Box<dyn BackendFactory>> {
    Ok(match backend {
        Backend::IosSim => Box::new(IosSimFactory),
        Backend::Mac => Box::new(MacFactory),
        Backend::Firefox => Box::new(FirefoxFactory),
        Backend::Chrome => Box::new(ChromeFactory),
        Backend::Safari => Box::new(SafariFactory),
    })
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
