use crate::{CHECKPOINT_SIGNATURE_SCHEME, IndexerCheckpointManifest};
use balance_history::{
    SNAPSHOT_SIGNATURE_SCHEME_ED25519, SnapshotManifest, SnapshotSigningKeyFile,
    SnapshotTrustedKeySet, signature_path_for_manifest_file,
};
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, Verifier};
use std::path::Path;

pub(crate) fn require_signing_key_trusted(
    signing_key_file: &SnapshotSigningKeyFile,
    trusted_keys_path: &Path,
) -> Result<(), String> {
    let expected = signing_key_file.to_signing_key()?.verifying_key();
    let trusted_keys = SnapshotTrustedKeySet::load(trusted_keys_path)?;
    let actual = trusted_keys
        .find_verifying_key(&signing_key_file.key_id)?
        .ok_or_else(|| {
            format!(
                "Checkpoint signing key {} is not present in trusted catalog {}",
                signing_key_file.key_id,
                trusted_keys_path.display()
            )
        })?;
    if actual != expected {
        return Err(format!(
            "Checkpoint signing key {} does not match trusted catalog {}",
            signing_key_file.key_id,
            trusted_keys_path.display()
        ));
    }
    Ok(())
}

pub(crate) fn sign_manifest(
    manifest: &IndexerCheckpointManifest,
    signing_key_file: &SnapshotSigningKeyFile,
    signature_path: &Path,
) -> Result<(), String> {
    if manifest.signature_scheme != CHECKPOINT_SIGNATURE_SCHEME {
        return Err(format!(
            "Unsupported checkpoint signature scheme {}",
            manifest.signature_scheme
        ));
    }
    if manifest.signing_key_id != signing_key_file.key_id {
        return Err(format!(
            "Checkpoint signer mismatch: manifest={}, key={}",
            manifest.signing_key_id, signing_key_file.key_id
        ));
    }
    let signing_key = signing_key_file.to_signing_key()?;
    let signature = signing_key.sign(&manifest.canonical_bytes()?);
    let encoded = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
    std::fs::write(signature_path, encoded).map_err(|error| {
        format!(
            "Failed to write checkpoint signature {}: {error}",
            signature_path.display()
        )
    })
}

pub(crate) fn verify_manifest_signature(
    manifest: &IndexerCheckpointManifest,
    signature_path: &Path,
    trusted_keys_path: &Path,
) -> Result<(), String> {
    if manifest.signature_scheme != CHECKPOINT_SIGNATURE_SCHEME {
        return Err(format!(
            "Unsupported checkpoint signature scheme {}",
            manifest.signature_scheme
        ));
    }
    let trusted_keys = SnapshotTrustedKeySet::load(trusted_keys_path)?;
    let verifying_key = trusted_keys
        .find_verifying_key(&manifest.signing_key_id)?
        .ok_or_else(|| {
            format!(
                "Checkpoint signer {} is not trusted by {}",
                manifest.signing_key_id,
                trusted_keys_path.display()
            )
        })?;
    let encoded = std::fs::read_to_string(signature_path).map_err(|error| {
        format!(
            "Failed to read checkpoint signature {}: {error}",
            signature_path.display()
        )
    })?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim().as_bytes())
        .map_err(|error| {
            format!(
                "Failed to decode checkpoint signature {}: {error}",
                signature_path.display()
            )
        })?;
    let bytes: [u8; 64] = raw.as_slice().try_into().map_err(|_| {
        format!(
            "Invalid checkpoint signature length in {}: expected 64, got {}",
            signature_path.display(),
            raw.len()
        )
    })?;
    verifying_key
        .verify(&manifest.canonical_bytes()?, &Signature::from_bytes(&bytes))
        .map_err(|error| {
            format!(
                "Checkpoint signature verification failed for signer {}: {error}",
                manifest.signing_key_id
            )
        })
}

pub(crate) fn verify_balance_history_manifest_signature(
    manifest: &SnapshotManifest,
    manifest_path: &Path,
    trusted_keys_path: &Path,
) -> Result<(), String> {
    if manifest.signature_scheme.as_deref() != Some(SNAPSHOT_SIGNATURE_SCHEME_ED25519) {
        return Err(format!(
            "Balance-history manifest {} is not signed with Ed25519",
            manifest_path.display()
        ));
    }
    let key_id = manifest
        .signing_key_id
        .as_deref()
        .ok_or_else(|| "Balance-history manifest has no signing_key_id".to_string())?;
    let trusted_keys = SnapshotTrustedKeySet::load(trusted_keys_path)?;
    let verifying_key = trusted_keys.find_verifying_key(key_id)?.ok_or_else(|| {
        format!(
            "Balance-history snapshot signer {key_id} is not trusted by {}",
            trusted_keys_path.display()
        )
    })?;
    let signature_path = signature_path_for_manifest_file(manifest_path);
    let encoded = std::fs::read_to_string(&signature_path).map_err(|error| {
        format!(
            "Failed to read balance-history signature {}: {error}",
            signature_path.display()
        )
    })?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim().as_bytes())
        .map_err(|error| {
            format!(
                "Failed to decode balance-history signature {}: {error}",
                signature_path.display()
            )
        })?;
    let bytes: [u8; 64] = raw.as_slice().try_into().map_err(|_| {
        format!(
            "Invalid balance-history signature length in {}: expected 64, got {}",
            signature_path.display(),
            raw.len()
        )
    })?;
    verifying_key
        .verify(&manifest.canonical_bytes()?, &Signature::from_bytes(&bytes))
        .map_err(|error| {
            format!("Balance-history signature verification failed for signer {key_id}: {error}")
        })
}
