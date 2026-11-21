//! Application values converted from Owly's validated protocol messages.
use owly_airdrop::proto::messages::{AskRequest, Offer};

/// Normalize a browser URL from a peer before presenting or opening it.
/// Windows can dispatch custom schemes and local file paths to other programs.
pub fn web_url(value: &str) -> Option<String> {
    let value = value.trim();
    if value
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return None;
    }
    let (scheme, _) = value.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let url = url::Url::parse(value).ok()?;
    (url.has_host() && url.username().is_empty() && url.password().is_none()).then(|| url.into())
}

/// A share item can include a title alongside its URL.
/// Preserve the order of web links and ignore app-specific URLs.
pub fn web_links(items: &[String]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    items
        .iter()
        .flat_map(|item| links_in_item(item))
        .filter(|url| seen.insert(url.clone()))
        .collect()
}

pub(crate) fn links_in_item(item: &str) -> impl Iterator<Item = String> + '_ {
    item.split_whitespace().filter_map(|part| {
        web_url(part).or_else(|| {
            let (_, url) = part
                .split_once(['】', '》'])
                .filter(|(title, _)| !title.contains("://"))?;
            web_url(url)
        })
    })
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingFile {
    pub name: String,
    pub kind: String,
    pub size: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingTransfer {
    pub id: String,
    pub sender_name: String,
    pub sender_model: Option<String>,
    pub files: Vec<IncomingFile>,
    pub urls: Vec<String>,
    pub icon: Option<Vec<u8>>,
    pub sender_record_data: Option<Vec<u8>>,
    /// Leaf of the client TLS chain, after the handshake proved key possession.
    /// The peer may send no certificate, or a certificate shared by several devices.
    pub sender_certificate_sha256: Option<[u8; 32]>,
}
impl IncomingTransfer {
    pub(crate) fn from_ask(ask: &AskRequest) -> Self {
        let (files, urls) = match &ask.offer {
            Offer::Files(files) => (
                files
                    .iter()
                    .map(|f| IncomingFile {
                        name: f.name.clone(),
                        kind: f.file_type.clone(),
                        size: f.size,
                    })
                    .collect(),
                ask.file_share_items.clone(),
            ),
            Offer::Urls(urls) => (Vec::new(), urls.clone()),
        };
        Self {
            id: ask.transfer_id.clone().unwrap_or_else(|| {
                format!(
                    "incoming-{}",
                    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                )
            }),
            sender_name: ask.sender_computer_name.clone(),
            sender_model: (!ask.sender_model_name.is_empty())
                .then(|| ask.sender_model_name.clone()),
            files,
            urls,
            icon: ask.file_icon.clone(),
            sender_record_data: ask.sender_record.clone(),
            sender_certificate_sha256: None,
        }
    }

    /// A pin can authorize writing files, while links always need consent.
    pub fn file_offer_certificate(&self) -> Option<[u8; 32]> {
        if self.urls.is_empty() && !self.files.is_empty() {
            self.sender_certificate_sha256
        } else {
            None
        }
    }
}
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_punctuation_inside_urls_and_accepts_a_share_title() {
        let whole = "https://example.org/作品【上集】?v=1";
        let url = web_url(whole).unwrap();
        assert_eq!(
            web_links(&[whole.into(), format!("【标题】{whole}")]),
            [url]
        );
        assert!(web_links(&["file:///example】https://example.org".into()]).is_empty());
    }
}
