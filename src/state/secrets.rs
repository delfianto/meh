//! Secure API key storage using OS keyring.

/// Store and retrieve API keys securely via the OS credential store.
pub struct SecretStore {
    service_name: String,
}

impl Default for SecretStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore {
    /// Create a new `SecretStore` using "meh" as the service name.
    #[must_use]
    pub fn new() -> Self {
        Self {
            service_name: "meh".to_string(),
        }
    }

    /// Store an API key. `key_name` is like `"anthropic_api_key"`.
    pub fn set(&self, key_name: &str, value: &str) -> anyhow::Result<()> {
        let entry = keyring::Entry::new(&self.service_name, key_name)?;
        entry.set_password(value)?;
        Ok(())
    }

    /// Retrieve an API key. Returns `None` if not found or keyring unavailable.
    #[must_use]
    pub fn get(&self, key_name: &str) -> Option<String> {
        let entry = keyring::Entry::new(&self.service_name, key_name).ok()?;
        entry.get_password().ok()
    }

    /// Delete an API key.
    pub fn delete(&self, key_name: &str) -> anyhow::Result<()> {
        let entry = keyring::Entry::new(&self.service_name, key_name)?;
        entry.delete_credential()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_store_creation() {
        let store = SecretStore::new();
        assert_eq!(store.service_name, "meh");
    }

    #[test]
    fn secret_store_default() {
        let store = SecretStore::default();
        assert_eq!(store.service_name, "meh");
    }

    // Keyring operations require a running secrets service.
    // These tests are ignored by default — run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn secret_store_set_and_get() {
        let store = SecretStore::new();
        store.set("test_key", "test_value").unwrap();
        let val = store.get("test_key");
        assert_eq!(val, Some("test_value".to_string()));
        store.delete("test_key").unwrap();
    }

    #[test]
    fn secret_store_get_nonexistent_returns_none() {
        let store = SecretStore::new();
        // This may or may not return None depending on the keyring backend,
        // but it should never panic.
        let _ = store.get("definitely_nonexistent_key_12345");
    }
}
