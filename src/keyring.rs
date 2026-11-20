//! The ChatGPT connection is kept in the GNOME keyring (Secret Service), like Plyph does.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::future::Future;

const LABEL: &str = "en-Master — ChatGPT connection";

fn attributes() -> HashMap<&'static str, &'static str> {
    HashMap::from([("application", "enmaster"), ("kind", "chatgpt-credentials")])
}

fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime").block_on(future)
}

fn legacy_attributes() -> HashMap<&'static str, &'static str> {
    HashMap::from([("application", crate::paths::LEGACY_NAME), ("kind", "chatgpt-credentials")])
}

pub fn read() -> Result<Option<String>> {
    block_on(async {
        let keyring = oo7::Keyring::new().await.context("could not open the GNOME keyring")?;
        keyring.unlock().await.context("the GNOME keyring is locked")?;
        let mut items = keyring.search_items(&attributes()).await?;
        let mut legacy = false;
        if items.is_empty() {
            items = keyring.search_items(&legacy_attributes()).await?;
            legacy = true;
        }
        let Some(item) = items.first() else { return Ok(None) };
        if item.is_locked().await? {
            item.unlock().await?;
        }
        let secret = item.secret().await?;
        let value = String::from_utf8(secret.as_bytes().to_vec()).context("keyring item is not text")?;
        if legacy {
            // Saved under the app's old name: store it under the new one and drop the old item.
            keyring.create_item(LABEL, &attributes(), value.as_str(), true).await?;
            item.delete().await?;
        }
        Ok(Some(value))
    })
}

pub fn write(value: &str) -> Result<()> {
    block_on(async {
        let keyring = oo7::Keyring::new().await.context("could not open the GNOME keyring")?;
        keyring.unlock().await.context("the GNOME keyring is locked")?;
        keyring
            .create_item(LABEL, &attributes(), value, true)
            .await
            .context("could not save the ChatGPT connection to the keyring")?;
        Ok(())
    })
}
