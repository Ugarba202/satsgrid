use crate::common::error::{Error, Result};
use crate::discovery::announcement::NodeAnnouncement;
use nostr_sdk::prelude::*;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Clone, Default)]
pub struct NostrDiscoveryClient {
    client: Client,
}

impl NostrDiscoveryClient {
    pub fn new() -> Self {
        Self {
            client: Client::default(),
        }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Add a relay to the client pool.
    pub async fn add_relay(&self, url: &str) -> Result<()> {
        self.client
            .add_relay(url)
            .await
            .map_err(|e| Error::Config(format!("Failed to add relay {}: {}", url, e)))?;
        Ok(())
    }

    /// Connect to all configured relays.
    pub async fn connect(&self) {
        self.client.connect().await;
    }

    /// Disconnect and shutdown the client pool.
    pub async fn shutdown(&self) {
        self.client.shutdown().await;
    }

    /// Fetch announced nodes from the client's connected relays.
    pub async fn fetch_announcements(
        &self,
        model: Option<&str>,
        timeout: Duration,
    ) -> Result<Vec<NodeAnnouncement>> {
        let filter = NodeAnnouncement::create_filter(model);
        let events = self
            .client
            .fetch_events(filter)
            .timeout(timeout)
            .await
            .map_err(|e| Error::Config(format!("Failed to fetch discovery events: {}", e)))?;

        let mut discovered = HashMap::new();
        for event in events {
            if let Ok(announcement) = NodeAnnouncement::from_event(&event) {
                let key = announcement
                    .pubkey
                    .clone()
                    .unwrap_or_else(|| announcement.node_url.clone());
                discovered.insert(key, announcement);
            }
        }

        Ok(discovered.into_values().collect())
    }

    /// Query relays for active SatsGrid node announcements matching optional model.
    pub async fn discover_nodes(
        relays: &[String],
        model: Option<&str>,
        timeout: Duration,
    ) -> Result<Vec<NodeAnnouncement>> {
        let client = Self::new();
        for r in relays {
            let _ = client.add_relay(r).await;
        }
        client.connect().await;

        let res = client.fetch_announcements(model, timeout).await;
        client.shutdown().await;
        res
    }

    /// Publish an announcement to the specified relays.
    pub async fn publish_announcement(
        relays: &[String],
        keys: &Keys,
        announcement: &NodeAnnouncement,
    ) -> Result<EventId> {
        let client = Self::new();
        for r in relays {
            let _ = client.add_relay(r).await;
        }
        client.connect().await;

        let builder = announcement.to_event_builder()?;
        let event = builder
            .finalize(keys)
            .map_err(|e| Error::Config(format!("Failed to sign announcement event: {}", e)))?;

        let event_id = event.id;
        client
            .client
            .send_event(&event)
            .await
            .map_err(|e| Error::Config(format!("Failed to publish announcement: {}", e)))?;

        client.shutdown().await;
        Ok(event_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_discovery_client_instantiation() {
        let client = NostrDiscoveryClient::new();
        assert!(!client.client().is_shutdown());
        client.shutdown().await;
        assert!(client.client().is_shutdown());
    }
}
