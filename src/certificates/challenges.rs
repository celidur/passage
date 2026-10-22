//! HTTP-01 proofs waiting to be fetched by the certificate authority.

use std::{
    collections::HashMap,
    sync::{Mutex, PoisonError},
};

/// Maps a challenge token to the key authorization served at
/// `/.well-known/acme-challenge/<token>`.
#[derive(Default)]
pub struct ChallengeStore(Mutex<HashMap<String, String>>);

impl ChallengeStore {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        // The map holds no invariant a panic could break.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn insert(&self, token: String, proof: String) {
        self.map().insert(token, proof);
    }

    pub fn remove(&self, token: &str) {
        self.map().remove(token);
    }

    pub fn get(&self, token: &str) -> Option<String> {
        self.map().get(token).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proofs_are_served_until_removed() {
        let store = ChallengeStore::default();
        assert!(store.get("token").is_none());
        store.insert("token".into(), "token.thumbprint".into());
        assert_eq!(store.get("token").as_deref(), Some("token.thumbprint"));
        store.remove("token");
        assert!(store.get("token").is_none());
    }
}
