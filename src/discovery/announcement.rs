use crate::common::error::{Error, Result};
use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};

pub const SATSGRID_DISCOVERY_KIND: u16 = 31990;
pub const SATSGRID_TAG: &str = "satsgrid";

/// Represents a public announcement published by an SatsGrid node on Nostr relays.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeAnnouncement {
    pub service: String,
    pub name: String,
    pub node_url: String,
    pub models: Vec<String>,
    pub pricing_sats: u64,
    pub network: String,
    pub version: String,
    #[serde(default)]
    pub pubkey: Option<String>,
}

impl NodeAnnouncement {
    pub fn new(
        name: impl Into<String>,
        node_url: impl Into<String>,
        models: Vec<String>,
        pricing_sats: u64,
        network: impl Into<String>,
    ) -> Self {
        Self {
            service: "satsgrid".to_string(),
            name: name.into(),
            node_url: node_url.into(),
            models,
            pricing_sats,
            network: network.into(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            pubkey: None,
        }
    }

    /// Converts the announcement into a signed Nostr EventBuilder (NIP-89 / Kind 31990).
    pub fn to_event_builder(&self) -> Result<EventBuilder> {
        let content = serde_json::to_string(self)
            .map_err(|e| Error::Config(format!("Failed to serialize node announcement: {}", e)))?;

        let mut tags = vec![
            Tag::identifier("satsgrid-node"),
            Tag::hashtag(SATSGRID_TAG),
            Tag::hashtag("ai-inference"),
            Tag::parse(["endpoint", &self.node_url])
                .map_err(|e| Error::Config(format!("Invalid endpoint tag: {}", e)))?,
            Tag::parse(["pricing", &self.pricing_sats.to_string()])
                .map_err(|e| Error::Config(format!("Invalid pricing tag: {}", e)))?,
            Tag::parse(["network", &self.network])
                .map_err(|e| Error::Config(format!("Invalid network tag: {}", e)))?,
        ];

        for model in &self.models {
            tags.push(
                Tag::parse(["m", model])
                    .map_err(|e| Error::Config(format!("Invalid model tag: {}", e)))?,
            );
        }

        Ok(EventBuilder::new(Kind::Custom(SATSGRID_DISCOVERY_KIND), content).tags(tags))
    }

    /// Parses a Nostr event into a NodeAnnouncement.
    pub fn from_event(event: &Event) -> Result<Self> {
        if event.kind != Kind::Custom(SATSGRID_DISCOVERY_KIND) {
            return Err(Error::Config(format!(
                "Unexpected event kind: {:?}, expected {}",
                event.kind, SATSGRID_DISCOVERY_KIND
            )));
        }

        let mut announcement: NodeAnnouncement = serde_json::from_str(&event.content)
            .map_err(|e| Error::Config(format!("Malformed announcement content: {}", e)))?;

        announcement.pubkey = Some(event.pubkey.to_hex());
        Ok(announcement)
    }

    /// Creates a Nostr subscription filter to query for active SatsGrid nodes.
    pub fn create_filter(model: Option<&str>) -> Filter {
        let mut filter = Filter::new()
            .kind(Kind::Custom(SATSGRID_DISCOVERY_KIND))
            .hashtag(SATSGRID_TAG);

        if let Some(m) = model {
            if let Ok(tag) = SingleLetterTag::from_char('m') {
                filter = filter.custom_tag(tag, m.to_string());
            }
        }

        filter
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_announcement_roundtrip() {
        let keys = Keys::generate();
        let announcement = NodeAnnouncement::new(
            "Test Node",
            "http://127.0.0.1:8080",
            vec!["llama3.2".to_string(), "mistral".to_string()],
            5,
            "regtest",
        );

        let builder = announcement
            .to_event_builder()
            .expect("builder should build");
        let event = builder
            .finalize(&keys)
            .expect("should sign and build event");

        let parsed = NodeAnnouncement::from_event(&event).expect("event should parse");
        assert_eq!(parsed.name, "Test Node");
        assert_eq!(parsed.node_url, "http://127.0.0.1:8080");
        assert_eq!(parsed.models, vec!["llama3.2", "mistral"]);
        assert_eq!(parsed.pricing_sats, 5);
        assert_eq!(parsed.network, "regtest");
        assert_eq!(parsed.pubkey, Some(keys.public_key().to_hex()));
    }

    #[test]
    fn test_filter_creation() {
        let filter = NodeAnnouncement::create_filter(Some("llama3.2"));
        assert!(filter.kinds.is_some());
    }
}
