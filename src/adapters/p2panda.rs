use std::io::{Cursor, Read};

use anyhow::{Context, Result, anyhow, bail};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload as AeadPayload},
};
use p2panda_core::{Body, Hash, Header, Operation, SigningKey, validate_operation};
use serde::{Deserialize, Serialize};

use crate::{
    domain::{CanonicalRecord, DocumentKey, ForgeDocumentUpdate, MAX_UPDATE_BYTES, RecordMetadata},
    ports::SecureLog,
};

pub const RECORD_KIND_LORO_UPDATE: u8 = 1;
pub const CODEC_RAW: u8 = 0;
pub const CODEC_ZSTD: u8 = 1;
pub const PROTOCOL_VERSION: u16 = 1;
pub const LORO_ENCODING_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommunityExtensions {
    pub protocol_version: u16,
    pub app_id: String,
    pub community_id: String,
    pub document_id: String,
    pub opaque_topic: [u8; 32],
    pub log_id: [u8; 32],
    pub document_shard: u16,
    pub record_kind: u8,
    pub schema_version: u32,
    pub auth_frontier: Vec<u8>,
    pub key_epoch: u32,
    pub codec_version: u8,
    pub loro_encoding_version: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdatePayload {
    pub document_id: String,
    pub loro_update: Vec<u8>,
    pub semantic_transaction: String,
}

#[derive(Clone, Debug, Serialize)]
struct EncryptionContext<'a> {
    extensions: &'a CommunityExtensions,
    author_key: [u8; 32],
    sequence: u32,
    backlink: Option<[u8; 32]>,
}

pub struct P2pandaSecureLog {
    signing_key: SigningKey,
    master_key: [u8; 32],
}

impl P2pandaSecureLog {
    pub fn new(signing_key: SigningKey, master_key: [u8; 32]) -> Self {
        Self {
            signing_key,
            master_key,
        }
    }
}

impl SecureLog for P2pandaSecureLog {
    fn author_key(&self) -> [u8; 32] {
        *self.signing_key.verifying_key().as_bytes()
    }

    fn document_log_id(&self, key: &DocumentKey) -> [u8; 32] {
        document_log_id(key)
    }

    fn forge_document_update(&self, input: ForgeDocumentUpdate<'_>) -> Result<CanonicalRecord> {
        forge_update(ForgeInput {
            key: input.key,
            signing_key: &self.signing_key,
            master_key: &self.master_key,
            sequence: input.sequence,
            backlink: input.backlink.map(Hash::from),
            schema_version: input.schema_version,
            key_epoch: input.key_epoch,
            auth_frontier: input.auth_frontier,
            loro_update: input.loro_update,
            semantic_transaction: input.semantic_transaction,
        })
    }
}

struct ForgeInput<'a> {
    pub key: &'a DocumentKey,
    pub signing_key: &'a SigningKey,
    pub master_key: &'a [u8; 32],
    pub sequence: u32,
    pub backlink: Option<Hash>,
    pub schema_version: u32,
    pub key_epoch: u32,
    pub auth_frontier: Vec<u8>,
    pub loro_update: &'a [u8],
    pub semantic_transaction: &'a str,
}

pub fn document_log_id(key: &DocumentKey) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key("community-stack/document-log/v1");
    add_part(&mut hasher, key.app_id.as_bytes());
    add_part(&mut hasher, key.community_id.as_bytes());
    add_part(&mut hasher, key.document_id.as_bytes());
    *hasher.finalize().as_bytes()
}

pub fn opaque_topic(key: &DocumentKey) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key("community-stack/opaque-topic/v1");
    add_part(&mut hasher, key.app_id.as_bytes());
    add_part(&mut hasher, key.community_id.as_bytes());
    *hasher.finalize().as_bytes()
}

fn add_part(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn derive_community_key(master_key: &[u8; 32], community_id: &str, epoch: u32) -> [u8; 32] {
    let mut material = Vec::with_capacity(32 + community_id.len() + 4);
    material.extend_from_slice(master_key);
    material.extend_from_slice(community_id.as_bytes());
    material.extend_from_slice(&epoch.to_be_bytes());
    blake3::derive_key("community-stack/community-content-key/v1", &material)
}

fn forge_update(input: ForgeInput<'_>) -> Result<CanonicalRecord> {
    if input.loro_update.len() > MAX_UPDATE_BYTES {
        bail!("Loro update exceeds {MAX_UPDATE_BYTES} byte limit");
    }

    let (encoded_update, codec_version) = compress_if_useful(input.loro_update)?;
    let payload = UpdatePayload {
        document_id: input.key.document_id.clone(),
        loro_update: encoded_update,
        semantic_transaction: input.semantic_transaction.to_owned(),
    };
    let payload_bytes = encode_cbor(&payload)?;

    let extensions = CommunityExtensions {
        protocol_version: PROTOCOL_VERSION,
        app_id: input.key.app_id.clone(),
        community_id: input.key.community_id.clone(),
        document_id: input.key.document_id.clone(),
        opaque_topic: opaque_topic(input.key),
        log_id: document_log_id(input.key),
        document_shard: u16::from_be_bytes([
            document_log_id(input.key)[0],
            document_log_id(input.key)[1],
        ]),
        record_kind: RECORD_KIND_LORO_UPDATE,
        schema_version: input.schema_version,
        auth_frontier: input.auth_frontier,
        key_epoch: input.key_epoch,
        codec_version,
        loro_encoding_version: LORO_ENCODING_VERSION,
    };

    let backlink_bytes = input.backlink.map(|hash| *hash.as_bytes());
    let context = EncryptionContext {
        extensions: &extensions,
        author_key: *input.signing_key.verifying_key().as_bytes(),
        sequence: input.sequence,
        backlink: backlink_bytes,
    };
    let aad = encode_cbor(&context)?;
    let ciphertext = encrypt(
        &derive_community_key(input.master_key, &input.key.community_id, input.key_epoch),
        &aad,
        &payload_bytes,
    )?;
    let body = Body::new(&ciphertext);
    let mut header = Header {
        version: 1,
        verifying_key: input.signing_key.verifying_key(),
        signature: None,
        payload_size: body.size(),
        payload_hash: Some(body.hash()),
        seq_num: input.sequence,
        backlink: input.backlink,
        extensions: extensions.clone(),
    };
    header.sign(input.signing_key);
    let operation = Operation {
        hash: header.hash(),
        header,
        body: Some(body),
    };
    validate_operation(&operation).context("self-validation of forged p2panda operation failed")?;

    Ok(CanonicalRecord {
        operation_hash: *operation.hash.as_bytes(),
        canonical_header: operation.header.to_bytes(),
        body_ciphertext: ciphertext,
        author_key: *operation.header.verifying_key.as_bytes(),
        log_id: extensions.log_id,
        sequence: input.sequence,
        backlink: backlink_bytes,
        metadata: RecordMetadata {
            app_id: extensions.app_id,
            community_id: extensions.community_id,
            document_id: extensions.document_id,
            record_kind: extensions.record_kind,
            codec_version: extensions.codec_version,
            key_epoch: extensions.key_epoch,
        },
    })
}

pub fn decode_update(
    header_bytes: &[u8],
    body_ciphertext: &[u8],
    master_key: &[u8; 32],
) -> Result<(Header<CommunityExtensions>, UpdatePayload)> {
    let header: Header<CommunityExtensions> = p2panda_core::cbor::decode_cbor(header_bytes)
        .context("invalid canonical p2panda header")?;
    let body = Body::new(body_ciphertext);
    let operation = Operation {
        hash: header.hash(),
        header: header.clone(),
        body: Some(body),
    };
    validate_operation(&operation).context("invalid p2panda operation")?;

    let context = EncryptionContext {
        extensions: &header.extensions,
        author_key: *header.verifying_key.as_bytes(),
        sequence: header.seq_num,
        backlink: header.backlink.map(|hash| *hash.as_bytes()),
    };
    let aad = encode_cbor(&context)?;
    let plaintext = decrypt(
        &derive_community_key(
            master_key,
            &header.extensions.community_id,
            header.extensions.key_epoch,
        ),
        &aad,
        body_ciphertext,
    )?;
    let mut payload: UpdatePayload = p2panda_core::cbor::decode_cbor(plaintext.as_slice())
        .context("invalid encrypted update payload")?;
    if payload.document_id != header.extensions.document_id {
        bail!("payload/header document mismatch");
    }
    payload.loro_update =
        decompress_bounded(&payload.loro_update, header.extensions.codec_version)?;
    Ok((header, payload))
}

fn encode_cbor<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(value, &mut bytes).context("CBOR encoding failed")?;
    Ok(bytes)
}

fn encrypt(key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce_bytes = [0_u8; 24];
    getrandom::fill(&mut nonce_bytes)
        .map_err(|error| anyhow!("OS random source failed: {error}"))?;
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce_bytes),
            AeadPayload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| anyhow!("payload encryption failed"))?;
    let mut output = Vec::with_capacity(24 + ciphertext.len());
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

fn decrypt(key: &[u8; 32], aad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    if ciphertext.len() < 40 {
        bail!("ciphertext is too short");
    }
    XChaCha20Poly1305::new(key.into())
        .decrypt(
            XNonce::from_slice(&ciphertext[..24]),
            AeadPayload {
                msg: &ciphertext[24..],
                aad,
            },
        )
        .map_err(|_| anyhow!("payload authentication failed"))
}

fn compress_if_useful(bytes: &[u8]) -> Result<(Vec<u8>, u8)> {
    if bytes.len() < 256 {
        return Ok((bytes.to_vec(), CODEC_RAW));
    }
    let compressed =
        zstd::stream::encode_all(Cursor::new(bytes), 3).context("zstd compression failed")?;
    if compressed.len() + 32 < bytes.len() {
        Ok((compressed, CODEC_ZSTD))
    } else {
        Ok((bytes.to_vec(), CODEC_RAW))
    }
}

fn decompress_bounded(bytes: &[u8], codec: u8) -> Result<Vec<u8>> {
    match codec {
        CODEC_RAW => {
            if bytes.len() > MAX_UPDATE_BYTES {
                bail!("raw update exceeds size limit");
            }
            Ok(bytes.to_vec())
        }
        CODEC_ZSTD => {
            let decoder = zstd::stream::read::Decoder::new(Cursor::new(bytes))
                .context("invalid zstd stream")?;
            let mut output = Vec::new();
            decoder
                .take((MAX_UPDATE_BYTES + 1) as u64)
                .read_to_end(&mut output)?;
            if output.len() > MAX_UPDATE_BYTES {
                bail!("decompressed update exceeds size limit");
            }
            Ok(output)
        }
        other => bail!("unsupported codec version {other}"),
    }
}
