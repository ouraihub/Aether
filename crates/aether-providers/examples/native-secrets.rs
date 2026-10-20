//! Real native credential smoke: run `seal`, then `open`, `tamper`, `forget` in separate processes.
//! Only an encrypted fixture is written; no user credential or master key is printed.

use std::path::Path;

use aether_providers::secrets::{protect, unprotect};
use aether_providers::{EndpointKeySave, EndpointSave, ProviderSave, Store};
use serde_json::json;

const PLAIN: &[u8] = b"\0\xffaether native persistence smoke\0";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: native-secrets seal|open|tamper|forget|unavailable|race <path>".into());
    }
    let file = Path::new(&args[1]);
    match args[0].to_str() {
        Some("race") => {
            std::fs::create_dir_all(file)?;
            let executable = std::env::current_exe()?;
            let paths: Vec<_> = (0..4)
                .map(|index| file.join(format!("{index}.blob")))
                .collect();
            let mut children = Vec::new();
            for path in &paths {
                children.push(
                    std::process::Command::new(&executable)
                        .arg("seal")
                        .arg(path)
                        .spawn()?,
                );
            }
            for mut child in children {
                assert!(child.wait()?.success(), "concurrent native seal failed");
            }
            for path in &paths {
                assert!(
                    std::process::Command::new(&executable)
                        .arg("open")
                        .arg(path)
                        .status()?
                        .success()
                );
                assert!(
                    std::process::Command::new(&executable)
                        .arg("tamper")
                        .arg(path)
                        .status()?
                        .success()
                );
                assert!(
                    std::process::Command::new(&executable)
                        .arg("forget")
                        .arg(path)
                        .status()?
                        .success()
                );
            }
        }
        Some("seal") => {
            let sealed = protect(PLAIN)?;
            assert!(!sealed.windows(PLAIN.len()).any(|bytes| bytes == PLAIN));
            assert_eq!(unprotect(&sealed)?.as_slice(), PLAIN);
            std::fs::write(file, sealed)?;
            seed_provider_store(file)?;
        }
        Some("open") => {
            assert_eq!(unprotect(&std::fs::read(file)?)?.as_slice(), PLAIN);
            let store = Store::open(&store_root(file))?;
            let (endpoint, key) = stored_key(&store)?;
            assert!(!endpoint.is_empty());
            assert_eq!(
                store.reveal_key(&key)?.as_str(),
                "native-provider-credential"
            );
        }
        Some("tamper") => {
            let sealed = std::fs::read(file)?;
            for index in 0..sealed.len() {
                let mut tampered = sealed.clone();
                tampered[index] ^= 1;
                assert!(unprotect(&tampered).is_err());
            }
            assert_eq!(unprotect(&sealed)?.as_slice(), PLAIN);
        }
        Some("forget") => {
            let sealed = std::fs::read(file)?;
            aether_support::secret::forget(&sealed)?;
            // Shared master keys cannot be deleted while another BLOB or backup needs them.
            assert_eq!(unprotect(&sealed)?.as_slice(), PLAIN);
            std::fs::remove_file(file)?;
            std::fs::remove_dir_all(store_root(file))?;
        }
        Some("unavailable") => {
            let before = std::fs::read(file)?;
            let error = protect(b"must-not-overwrite-existing-credential")
                .err()
                .ok_or("native store unexpectedly available")?;
            assert_eq!(error.details().reason(), Some("secret_store_unavailable"));
            assert!(!format!("{error:?}").contains("must-not-overwrite"));
            assert_eq!(std::fs::read(file)?, before);
            let store = Store::open(&store_root(file))?;
            let (endpoint, key) = stored_key(&store)?;
            let cipher = stored_cipher(file)?;
            let update: EndpointKeySave = serde_json::from_value(json!({
                "endpointId": endpoint, "id": key, "key": "must-not-overwrite-existing-credential"
            }))?;
            let error = store
                .save_key(update)
                .err()
                .ok_or("native overwrite unexpectedly succeeded")?;
            assert_eq!(error.details().reason(), Some("secret_store_unavailable"));
            assert_eq!(stored_cipher(file)?, cipher);
        }
        _ => return Err("unknown native smoke operation".into()),
    }
    Ok(())
}

fn store_root(file: &Path) -> std::path::PathBuf {
    file.with_extension("store")
}

fn stored_key(store: &Store) -> Result<(String, String), Box<dyn std::error::Error>> {
    let listed = store.list()?;
    let endpoint = &listed[0].endpoints[0];
    Ok((endpoint.id.clone(), endpoint.keys[0].id.clone()))
}

fn stored_cipher(file: &Path) -> Result<Vec<u8>, rusqlite::Error> {
    let database = rusqlite::Connection::open(store_root(file).join("providers.db"))?;
    database.query_row("SELECT key_ciphertext FROM endpoint_keys", [], |row| {
        row.get(0)
    })
}

fn seed_provider_store(file: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let root = store_root(file);
    std::fs::create_dir_all(&root)?;
    let store = Store::open(&root)?;
    let endpoint: EndpointSave = serde_json::from_value(json!({
        "slug": "native", "baseUrl": "https://example.com", "protocol": "openai-chat", "models": []
    }))?;
    let request: ProviderSave = serde_json::from_value(json!({
        "name": "native-smoke", "endpoints": []
    }))?;
    let listed = store.save(ProviderSave {
        endpoints: vec![endpoint],
        ..request
    })?;
    let key: EndpointKeySave = serde_json::from_value(json!({
        "endpointId": listed[0].endpoints[0].id, "key": "native-provider-credential"
    }))?;
    store.save_key(key)?;
    Ok(())
}
