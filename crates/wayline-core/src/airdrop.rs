//! Application values converted from Owly's validated protocol messages.
use owly_airdrop::proto::messages::{AskRequest, Offer};

const MAX_SHARE_TEXT_BYTES: usize = 16 * 1024;

/// Normalize a browser URL from a peer before presenting or opening it.
/// Windows can dispatch custom schemes and local file paths to other programs.
pub fn web_url(value: &str) -> Option<String> {
    let value = value.trim();
    if value
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '\\' || is_bidi_control(c))
    {
        return None;
    }
    let (scheme, remainder) = value.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = remainder.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let url = url::Url::parse(value).ok()?;
    if let url::Host::Domain(domain) = url.host()? {
        let domain = domain.strip_suffix('.').unwrap_or(domain);
        if domain.split('.').any(|label| {
            label.is_empty()
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        }) {
            return None;
        }
    }
    Some(url.into())
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
    item.split_whitespace().filter_map(link_in_word)
}

fn scheme_character(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.')
}

fn link_in_word(word: &str) -> Option<String> {
    // Examine the first scheme in this token. A URL embedded in another
    // scheme or in a web URL's query must not become a separate browser action.
    let (prefix_and_scheme, _) = word.split_once("://")?;
    let separator = prefix_and_scheme
        .char_indices()
        .rev()
        .find(|(_, ch)| !scheme_character(*ch));
    let Some((index, boundary)) = separator else {
        return web_url(word);
    };
    let prefix = &word[..index];
    if prefix.contains(':')
        || boundary == ':'
            && prefix.starts_with(|ch: char| ch.is_ascii_alphabetic())
            && prefix.chars().all(scheme_character)
    {
        return None;
    }

    let candidate = &word[index + boundary.len_utf8()..];
    let closer = match boundary {
        '<' => '>',
        '(' => ')',
        '[' => ']',
        '（' => '）',
        '【' => '】',
        '〖' => '〗',
        '《' => '》',
        '「' => '」',
        '『' => '』',
        '“' => '”',
        '‘' => '’',
        '"' | '\'' => boundary,
        '】' | '〗' | '》' | '」' | '』' | '”' | '’' | '：' | ':' | '，' | '、' | '；' =>
        {
            return web_url(candidate);
        }
        _ => return None,
    };
    web_url(candidate.strip_suffix(closer)?)
}

fn is_bidi_control(character: char) -> bool {
    matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

fn share_text(items: &[String]) -> Option<String> {
    let mut text = String::new();
    'items: for (index, item) in items.iter().enumerate() {
        if index > 0 {
            if text.len() == MAX_SHARE_TEXT_BYTES {
                break;
            }
            text.push('\n');
        }
        let mut characters = item.chars().peekable();
        while let Some(character) = characters.next() {
            let character = match character {
                '\r' => {
                    characters.next_if_eq(&'\n');
                    '\n'
                }
                '\n' | '\t' => character,
                other if other.is_control() || is_bidi_control(other) => continue,
                other => other,
            };
            if text.len() + character.len_utf8() > MAX_SHARE_TEXT_BYTES {
                break 'items;
            }
            text.push(character);
        }
    }
    (!text.trim().is_empty()).then_some(text)
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
    /// The shared text, with controls removed and a 16 KiB UTF-8 limit.
    pub share_text: Option<String>,
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
        let share_text = share_text(&urls);
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
            share_text,
            icon: ask.file_icon.clone(),
            sender_record_data: ask.sender_record.clone(),
            sender_certificate_sha256: None,
        }
    }

    /// A pin can authorize writing files, while links always need consent.
    pub fn file_offer_certificate(&self) -> Option<[u8; 32]> {
        if self.urls.is_empty() && self.share_text.is_none() && !self.files.is_empty() {
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
    }

    #[test]
    fn extracts_web_links_from_shared_prose_and_keeps_unicode_url_components() {
        let chinese_url = "https://例子.测试/作品【上集】?q=你好";
        let chinese = web_url(chinese_url).unwrap();
        assert!(chinese.starts_with("https://xn--fsqu00a.xn--0zwm56d/"));
        let short_url = "https://v.douyin.com/WaylineTest/";
        let second_url = "https://b23.tv/wayline-test";
        let items = [
            format!("3.64 复制打开抖音，看看〖示例的作品〗{short_url} 04/09"),
            format!("标题：‘{chinese_url}’\n【哔哩哔哩】{second_url}\n{short_url}"),
        ];
        assert_eq!(web_links(&items), [short_url, &chinese, second_url]);
        assert_eq!(
            web_links(&["查看:https://example.org/?next=https://other.invalid/".into()]),
            ["https://example.org/?next=https://other.invalid/"]
        );
    }

    #[test]
    fn ignores_other_schemes_and_requires_a_valid_http_authority() {
        let refused = [
            "custom://foo【https://example.org/path】",
            "file:///example】https://example.org",
            "file:C:\\docs】https://example.org",
            "(custom://test】https://example.org)",
            "bilibili:https://example.org/",
            "https:///example.org",
            "https://?redirect=https://example.org",
            "https://./",
            "https://bad..example.org/",
            "https://@example.org",
            "https://user:password@example.org/",
            "https://example.org/\u{202e}path",
        ];
        assert!(web_links(&refused.map(str::to_owned)).is_empty());
        assert_eq!(
            web_links(&["（https://[::1]:8443/docs） <http://localhost:8080/>".into()]),
            ["https://[::1]:8443/docs", "http://localhost:8080/"]
        );
    }

    #[test]
    fn share_text_preserves_prose_and_line_breaks_with_a_utf8_limit() {
        let items = [
            "视频标题\r\n\u{0000}\u{007f}\u{202e}https://example.org/👩‍💻\r文字\t".into(),
            "下一段".into(),
        ];
        assert_eq!(
            share_text(&items).as_deref(),
            Some("视频标题\nhttps://example.org/👩‍💻\n文字\t\n下一段")
        );
        assert_eq!(share_text(&["\r\t \u{0007}\u{202e}".into()]), None);

        let prefix = "字".repeat(MAX_SHARE_TEXT_BYTES / '字'.len_utf8());
        let long = format!("{prefix}🙂后文");
        assert_eq!(share_text(&[long]), Some(prefix));
    }
}
