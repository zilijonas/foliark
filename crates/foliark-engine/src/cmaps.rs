//! The predefined CMap registry (spec 10.3). The wasm carries no CMaps: the SDK fetches the versioned pack files a
//! document needs (`DocumentInfo.requiredAssets`) and hands their bytes here before the first page is read. The
//! interpreter asks for `&'static [u8]`, so each registered map is leaked once; the set of names is closed
//! (hayro's predefined list) and a name registers once, which bounds the memory at the size of the pack.
use foliark_model::{ErrorCode, ErrorDetails, PdfError};
use hayro::hayro_interpret::hayro_cmap::{CMap, CMapName};
use std::collections::BTreeMap;
use std::sync::RwLock;

/// Largest pack file is 0.33 MB (UniCNS-UCS2-H); anything bigger is not a CMap of the pack.
pub const MAX_CMAP_BYTES: usize = 1 << 20;

static REGISTRY: RwLock<BTreeMap<String, &'static [u8]>> = RwLock::new(BTreeMap::new());

/// A name the pack can supply: predefined by hayro-cmap, and not Identity (which is built in).
pub fn is_predefined(name: &str) -> bool {
    !matches!(
        CMapName::from_bytes(name.as_bytes()),
        CMapName::Custom(_) | CMapName::IdentityH | CMapName::IdentityV
    )
}

/// Bytes per code of a Type0 `/Encoding` name whose codespace is fixed: Identity-H/V and the `Uni*-UCS2-*` CMaps are
/// two-byte. Every other predefined CMap mixes code lengths (or is not known to), so a rewrite of its strings is refused.
pub fn fixed_code_width(encoding: &str) -> Option<u8> {
    let ucs2 = encoding.starts_with("Uni") && encoding.contains("-UCS2-");
    (matches!(encoding, "Identity-H" | "Identity-V") || ucs2).then_some(2)
}

/// The interpreter's `cmap_resolver`.
pub(crate) fn resolve(name: CMapName<'_>) -> Option<&'static [u8]> {
    let name = std::str::from_utf8(name.to_bytes()).ok()?;
    REGISTRY.read().ok()?.get(name).copied()
}

/// Registers one pack file. A map `usecmap`s another (`UniJIS-UCS2-V` uses `UniJIS-UCS2-H`), so register the
/// base first. Corrupt, truncated or mislabelled bytes are `ASSET_LOAD_FAILED` with `details.feature =
/// "cmap:<name>"` (never the bytes); a map already
/// registered is kept (pack files are versioned, so the same name is the same data).
pub fn register(name: &str, bytes: &[u8]) -> Result<(), PdfError> {
    let bad = || {
        let feature = Some(format!("cmap:{name}"));
        PdfError::with(
            ErrorCode::AssetLoadFailed,
            ErrorDetails {
                feature,
                ..Default::default()
            },
        )
    };
    if !is_predefined(name) || bytes.len() > MAX_CMAP_BYTES {
        return Err(bad());
    }
    let complete = bytes
        .windows(13)
        .rev()
        .take(256)
        .any(|w| w == b"%%EndResource");
    let parsed = complete.then(|| CMap::parse(bytes, resolve)).flatten();
    if parsed.is_none_or(|m| m.metadata().name.as_deref() != Some(name.as_bytes())) {
        return Err(bad());
    }
    let mut all = REGISTRY.write().map_err(|_| bad())?;
    all.entry(name.to_owned())
        .or_insert_with(|| Box::leak(bytes.to_vec().into_boxed_slice()));
    Ok(())
}

/// An export rewrites fonts from what the interpreter read, and without a document's CMaps it reads nothing: it would
/// drop the glyphs it cannot see. So a native export of a document whose `requiredAssets` are not all registered is
/// refused, naming the first missing map (`ASSET_LOAD_FAILED`, `details.feature = "cmap:<name>"`).
pub fn require(assets: &[String]) -> Result<(), PdfError> {
    let all = REGISTRY
        .read()
        .map_err(|_| PdfError::new(ErrorCode::Internal))?;
    let missing = assets.iter().find(|a| {
        a.strip_prefix("cmap:")
            .is_some_and(|n| !all.contains_key(n))
    });
    missing.map_or(Ok(()), |m| {
        let feature = Some(m.clone());
        Err(PdfError::with(
            ErrorCode::AssetLoadFailed,
            ErrorDetails {
                feature,
                ..Default::default()
            },
        ))
    })
}
