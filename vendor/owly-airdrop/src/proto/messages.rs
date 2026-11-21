//! The `/Discover` and `/Ask` plist bodies (§6.3, §6.4).
//!
//! Decoding is lenient where the spec lets it be: unknown keys are ignored,
//! and both `/Ask` shapes are accepted. Encoding always produces the shape
//! iOS 18 needs (ASK-7).

use std::fmt;

use super::archive::extract::RelPath;
use super::plist::{Dict, Value};

/// `StatusCode` values in a `/Discover` response.
pub const STATUS_EVERYONE: i64 = 100;
pub const STATUS_CONTACT: i64 = 200;
pub const STATUS_NOT_A_CONTACT: i64 = 401;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Invalid(&'static str),
    MixedOffer(MixedOfferShape),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(reason) => write!(f, "bad message: {reason}"),
            Self::MixedOffer(shape) => write!(f, "bad message: {shape}"),
        }
    }
}

impl std::error::Error for Error {}

fn dict(value: &Value) -> Result<&Dict, Error> {
    value.as_dict().ok_or(Error::Invalid("not a dictionary"))
}

fn string(d: &Dict, key: &str) -> Result<Option<String>, Error> {
    match d.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(Error::Invalid("expected a string")),
    }
}

fn data(d: &Dict, key: &str) -> Result<Option<Vec<u8>>, Error> {
    match d.get(key) {
        None => Ok(None),
        Some(Value::Data(b)) => Ok(Some(b.clone())),
        Some(_) => Err(Error::Invalid("expected data")),
    }
}

fn insert_opt(d: &mut Dict, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        d.insert(key.into(), value);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoverRequest {
    pub sender_record: Option<Vec<u8>>,
    pub sender_computer_name: Option<String>,
    pub sender_model_name: Option<String>,
    pub sender_id: Option<String>,
}

impl DiscoverRequest {
    pub fn from_plist(value: &Value) -> Result<Self, Error> {
        let d = dict(value)?;
        Ok(DiscoverRequest {
            sender_record: data(d, "SenderRecordData")?,
            sender_computer_name: string(d, "SenderComputerName")?,
            sender_model_name: string(d, "SenderModelName")?,
            sender_id: string(d, "SenderID")?,
        })
    }

    pub fn to_plist(&self) -> Value {
        let mut d = Dict::new();
        insert_opt(
            &mut d,
            "SenderRecordData",
            self.sender_record.clone().map(Value::Data),
        );
        insert_opt(
            &mut d,
            "SenderComputerName",
            self.sender_computer_name.clone().map(Value::String),
        );
        insert_opt(
            &mut d,
            "SenderModelName",
            self.sender_model_name.clone().map(Value::String),
        );
        insert_opt(
            &mut d,
            "SenderID",
            self.sender_id.clone().map(Value::String),
        );
        Value::Dict(d)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoverResponse {
    pub status_code: Option<i64>,
    /// Present exactly when the sender may list this receiver (DISC-1).
    pub receiver_computer_name: Option<String>,
    pub receiver_model_name: Option<String>,
    /// UTF-8 JSON (DISC-3).
    pub receiver_media_capabilities: Option<Vec<u8>>,
    pub receiver_record: Option<Vec<u8>>,
}

impl DiscoverResponse {
    pub fn from_plist(value: &Value) -> Result<Self, Error> {
        let d = dict(value)?;
        Ok(DiscoverResponse {
            status_code: d.get("StatusCode").and_then(Value::as_integer),
            receiver_computer_name: string(d, "ReceiverComputerName")?,
            receiver_model_name: string(d, "ReceiverModelName")?,
            receiver_media_capabilities: data(d, "ReceiverMediaCapabilities")?,
            receiver_record: data(d, "ReceiverRecordData")?,
        })
    }

    pub fn to_plist(&self) -> Value {
        let mut d = Dict::new();
        insert_opt(&mut d, "StatusCode", self.status_code.map(Value::Integer));
        insert_opt(
            &mut d,
            "ReceiverComputerName",
            self.receiver_computer_name.clone().map(Value::String),
        );
        insert_opt(
            &mut d,
            "ReceiverModelName",
            self.receiver_model_name.clone().map(Value::String),
        );
        insert_opt(
            &mut d,
            "ReceiverMediaCapabilities",
            self.receiver_media_capabilities.clone().map(Value::Data),
        );
        insert_opt(
            &mut d,
            "ReceiverRecordData",
            self.receiver_record.clone().map(Value::Data),
        );
        Value::Dict(d)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskFile {
    pub name: String,
    /// A UTI such as `public.jpeg`.
    pub file_type: String,
    /// Uncompressed size. Required in the iOS 18 shape, absent in the legacy
    /// one.
    pub size: Option<u64>,
    /// The entry's path in the `/Upload` archive, `./<name>`.
    pub bom_path: String,
    pub is_directory: bool,
}

/// The structure of a rejected mixed offer, without any values from the sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MixedOfferShape {
    transfer_type: TransferTypeKind,
    files: usize,
    items: usize,
    shortcut_files: usize,
    empty_link_metadata_files: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TransferTypeKind {
    Files,
    Links,
    Missing,
    Other,
}

impl TransferTypeKind {
    fn from_plist(value: Option<&Value>) -> Self {
        match value {
            None => Self::Missing,
            Some(Value::Dict(kind)) if kind.len() == 1 => match kind.first_key_value() {
                Some((key, Value::Dict(_))) if key == "files" => Self::Files,
                Some((key, Value::Dict(options))) if key == "links" && options.is_empty() => {
                    Self::Links
                }
                _ => Self::Other,
            },
            _ => Self::Other,
        }
    }
}

impl MixedOfferShape {
    fn new(transfer_type: TransferTypeKind, files: &[AskFile], items: usize) -> Self {
        let shortcut_files = files
            .iter()
            .filter(|file| {
                !file.is_directory
                    && (matches!(
                        file.file_type.as_str(),
                        "public.url"
                            | "com.apple.web-internet-location"
                            | "com.apple.internet-location"
                    ) || has_shortcut_extension(&file.name))
            })
            .count();
        Self {
            transfer_type,
            files: files.len(),
            items,
            shortcut_files,
            empty_link_metadata_files: files
                .iter()
                .filter(|file| is_empty_web_link_metadata(file))
                .count(),
        }
    }
}

fn has_shortcut_extension(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        extension.eq_ignore_ascii_case("webloc") || extension.eq_ignore_ascii_case("url")
    })
}

fn is_empty_web_link_metadata(file: &AskFile) -> bool {
    !file.is_directory
        && file.size == Some(0)
        && has_shortcut_extension(&file.name)
        && matches!(
            file.file_type.as_str(),
            "public.url" | "com.apple.web-internet-location"
        )
        && RelPath::file_name(&file.name)
            .is_ok_and(|name| RelPath::parse(&file.bom_path).is_ok_and(|path| path == name))
}

impl fmt::Display for MixedOfferShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.transfer_type {
            TransferTypeKind::Files => "files",
            TransferTypeKind::Links => "links",
            TransferTypeKind::Missing => "missing",
            TransferTypeKind::Other => "other",
        };
        write!(
            f,
            "both files and URLs offered (transfer_type={kind}, files={}, items={}, shortcut_files={}, empty_link_metadata_files={})",
            self.files, self.items, self.shortcut_files, self.empty_link_metadata_files
        )
    }
}

/// What the `/Ask` offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    Files(Vec<AskFile>),
    Urls(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskRequest {
    /// `TransferID.id`, the UUID that ties `/Upload` to this request (UP-9).
    /// `None` for the legacy shape.
    pub transfer_id: Option<String>,
    /// The key of the iOS 18 shape's `TransferType` dict, such as `files`
    /// or `links` (docs/open-questions.md, item 12). Encoding derives it
    /// from the offer when it's `None`.
    pub transfer_type: Option<String>,
    pub sender_computer_name: String,
    pub sender_model_name: String,
    pub sender_id: String,
    pub bundle_id: String,
    pub sender_record: Option<Vec<u8>>,
    pub file_icon: Option<Vec<u8>>,
    pub offer: Offer,
    /// Text and links that accompany an explicitly declared file upload.
    pub file_share_items: Vec<String>,
    /// An explicit links offer carried a zero-byte URL descriptor for each item.
    pub has_link_metadata: bool,
    /// The whole request as received, for logs ([`Value::describe`]).
    /// Empty for one built to send.
    pub described: String,
}

impl AskRequest {
    /// Decodes either shape. Required fields that are missing default to
    /// empty, so an odd sender still gets a prompt; only `Files`/`Items`
    /// decide whether the request makes sense.
    pub fn from_plist(value: &Value) -> Result<Self, Error> {
        let d = dict(value)?;
        let transfer_id = match d.get("TransferID") {
            None => None,
            Some(Value::Dict(t)) => {
                Some(string(t, "id")?.ok_or(Error::Invalid("TransferID has no id"))?)
            }
            // A plain string doesn't get a prompt on iOS 18 (ASK-7), but
            // there's no reason to refuse it here.
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return Err(Error::Invalid("bad TransferID")),
        };
        let transfer_type = match d.get("TransferType") {
            Some(Value::Dict(t)) => t.keys().next().cloned(),
            _ => None,
        };
        let files = match d.get("Files") {
            None => Vec::new(),
            Some(Value::Array(files)) => files.iter().map(ask_file).collect::<Result<_, _>>()?,
            Some(_) => return Err(Error::Invalid("Files is not an array")),
        };
        let items = match d.get("Items") {
            None => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|i| {
                    i.as_str()
                        .map(String::from)
                        .ok_or(Error::Invalid("Items holds a non-string"))
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(Error::Invalid("Items is not an array")),
        };
        let mut has_link_metadata = false;
        let mut file_share_items = Vec::new();
        let offer = match (files.is_empty(), items.is_empty()) {
            (false, true) => Offer::Files(files),
            (true, false) => Offer::Urls(items),
            (true, true) => return Err(Error::Invalid("nothing offered")),
            (false, false) => {
                let kind = TransferTypeKind::from_plist(d.get("TransferType"));
                match kind {
                    TransferTypeKind::Files => {
                        file_share_items = items;
                        Offer::Files(files)
                    }
                    TransferTypeKind::Links
                        if files.len() == items.len()
                            && files.iter().all(is_empty_web_link_metadata) =>
                    {
                        has_link_metadata = true;
                        Offer::Urls(items)
                    }
                    _ => {
                        return Err(Error::MixedOffer(MixedOfferShape::new(
                            kind,
                            &files,
                            items.len(),
                        )));
                    }
                }
            }
        };
        let describe = !has_link_metadata && file_share_items.is_empty();
        Ok(AskRequest {
            transfer_id,
            transfer_type,
            sender_computer_name: string(d, "SenderComputerName")?.unwrap_or_default(),
            sender_model_name: string(d, "SenderModelName")?.unwrap_or_default(),
            sender_id: string(d, "SenderID")?.unwrap_or_default(),
            bundle_id: string(d, "BundleID")?.unwrap_or_default(),
            sender_record: data(d, "SenderRecordData")?,
            file_icon: data(d, "FileIcon")?,
            offer,
            file_share_items,
            has_link_metadata,
            described: if describe {
                value.describe()
            } else {
                String::new()
            },
        })
    }

    /// The iOS 18 shape (ASK-7), the only one senders send. The legacy
    /// shape is only decoded.
    pub fn to_plist(&self) -> Value {
        let mut d = Dict::new();
        if let Some(id) = &self.transfer_id {
            d.insert(
                "TransferID".into(),
                Value::Dict(Dict::from([("id".into(), id.clone().into())])),
            );
        }
        // `links` for a URL share hasn't been captured from an Apple
        // sender (docs/open-questions.md, item 12).
        let kind = match (&self.transfer_type, &self.offer) {
            (Some(kind), _) => kind.as_str(),
            (None, Offer::Files(_)) => "files",
            (None, Offer::Urls(_)) => "links",
        };
        d.insert(
            "TransferType".into(),
            Value::Dict(Dict::from([(kind.into(), Value::Dict(Dict::new()))])),
        );
        d.insert(
            "SenderComputerName".into(),
            self.sender_computer_name.clone().into(),
        );
        d.insert(
            "SenderModelName".into(),
            self.sender_model_name.clone().into(),
        );
        d.insert("SenderID".into(), self.sender_id.clone().into());
        d.insert("BundleID".into(), self.bundle_id.clone().into());
        d.insert("ConvertMediaFormats".into(), false.into());
        insert_opt(
            &mut d,
            "SenderRecordData",
            self.sender_record.clone().map(Value::Data),
        );
        insert_opt(&mut d, "FileIcon", self.file_icon.clone().map(Value::Data));
        let (files, items) = match &self.offer {
            Offer::Files(files) => (files.as_slice(), self.file_share_items.as_slice()),
            Offer::Urls(urls) => (&[][..], urls.as_slice()),
        };
        let files = files
            .iter()
            .map(|f| {
                let mut e = Dict::new();
                e.insert("FileName".into(), f.name.clone().into());
                e.insert("FileType".into(), f.file_type.clone().into());
                e.insert("FileBomPath".into(), f.bom_path.clone().into());
                e.insert("FileIsDirectory".into(), f.is_directory.into());
                e.insert(
                    "FileSize".into(),
                    Value::Integer(f.size.unwrap_or(0) as i64),
                );
                e.insert("ShouldConvertMediaFormats".into(), false.into());
                Value::Dict(e)
            })
            .collect();
        d.insert("Files".into(), Value::Array(files));
        d.insert(
            "Items".into(),
            Value::Array(items.iter().map(|u| u.clone().into()).collect()),
        );
        Value::Dict(d)
    }
}

fn ask_file(value: &Value) -> Result<AskFile, Error> {
    let d = dict(value)?;
    let name = string(d, "FileName")?.ok_or(Error::Invalid("file without a FileName"))?;
    let bom_path = string(d, "FileBomPath")?.unwrap_or_else(|| format!("./{name}"));
    let size = match d.get("FileSize") {
        None => None,
        Some(Value::Integer(n)) if *n >= 0 => Some(*n as u64),
        Some(_) => return Err(Error::Invalid("bad FileSize")),
    };
    let is_directory = match d.get("FileIsDirectory") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(Error::Invalid("bad FileIsDirectory")),
    };
    Ok(AskFile {
        file_type: string(d, "FileType")?.unwrap_or_else(|| "public.data".into()),
        name,
        size,
        bom_path,
        is_directory,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AskResponse {
    pub receiver_computer_name: String,
    pub receiver_model_name: String,
}

impl AskResponse {
    pub fn to_plist(&self) -> Value {
        Value::Dict(Dict::from([
            (
                "ReceiverComputerName".into(),
                self.receiver_computer_name.clone().into(),
            ),
            (
                "ReceiverModelName".into(),
                self.receiver_model_name.clone().into(),
            ),
        ]))
    }
}
