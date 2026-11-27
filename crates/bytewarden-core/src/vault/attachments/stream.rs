use std::{io::SeekFrom, path::Path};

use aes::Aes256;
use bitwarden_crypto::SymmetricCryptoKey;
use cbc::cipher::{block_padding::Pkcs7, Block, BlockModeDecrypt, BlockModeEncrypt, KeyIvInit};
use hmac::{Hmac, KeyInit, Mac};
use rand::{rngs::OsRng, RngCore};
use sha2::Sha256;
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};
use zeroize::Zeroizing;

use super::{download_temporary, VaultError};

pub(super) const LEGACY_FORMAT: u8 = 2;
const BLOCK_BYTES: usize = 16;
const CHUNK_BYTES: usize = 64 * 1024;
const HEADER_BYTES: usize = 1 + BLOCK_BYTES + 32;

type AttachmentMac = Hmac<Sha256>;

pub(super) async fn encrypt(
    mut source: File,
    len: u64,
    key: &SymmetricCryptoKey,
) -> Result<(File, u64), VaultError> {
    let key = key.to_encoded();
    let (encryption_key, mac_key) = key.as_ref().split_at(32);
    let mut iv = [0; BLOCK_BYTES];
    OsRng
        .try_fill_bytes(&mut iv)
        .map_err(|_| VaultError::AttachmentUploadFailed)?;
    let mut cipher = cbc::Encryptor::<Aes256>::new_from_slices(encryption_key, &iv)
        .map_err(|_| VaultError::AttachmentUploadFailed)?;
    let mut mac =
        AttachmentMac::new_from_slice(mac_key).map_err(|_| VaultError::AttachmentUploadFailed)?;
    mac.update(&iv);

    let mut output = File::from_std(tempfile::tempfile().map_err(|_| VaultError::FileAccess)?);
    output
        .seek(SeekFrom::Start(HEADER_BYTES as u64))
        .await
        .map_err(|_| VaultError::FileAccess)?;
    let mut buffer = Zeroizing::new(vec![0; CHUNK_BYTES]);
    let mut remaining = len;
    while remaining >= CHUNK_BYTES as u64 {
        source
            .read_exact(&mut buffer)
            .await
            .map_err(|_| VaultError::AttachmentFileRead)?;
        cipher.encrypt_blocks(Block::<Aes256>::slice_as_chunks_mut(&mut buffer).0);
        mac.update(&buffer);
        output
            .write_all(&buffer)
            .await
            .map_err(|_| VaultError::FileAccess)?;
        remaining -= CHUNK_BYTES as u64;
    }
    source
        .read_exact(&mut buffer[..remaining as usize])
        .await
        .map_err(|_| VaultError::AttachmentFileRead)?;
    if source
        .read(&mut Zeroizing::new([0u8; 1])[..])
        .await
        .map_err(|_| VaultError::AttachmentFileRead)?
        != 0
    {
        return Err(VaultError::AttachmentFileRead);
    }
    let tail = cipher
        .encrypt_padded::<Pkcs7>(&mut buffer, remaining as usize)
        .map_err(|_| VaultError::AttachmentUploadFailed)?;
    mac.update(tail);
    output
        .write_all(tail)
        .await
        .map_err(|_| VaultError::FileAccess)?;

    let mut header = [0; HEADER_BYTES];
    header[0] = LEGACY_FORMAT;
    header[1..1 + BLOCK_BYTES].copy_from_slice(&iv);
    header[1 + BLOCK_BYTES..].copy_from_slice(&mac.finalize().into_bytes());
    output
        .seek(SeekFrom::Start(0))
        .await
        .map_err(|_| VaultError::FileAccess)?;
    output
        .write_all(&header)
        .await
        .map_err(|_| VaultError::FileAccess)?;
    output.flush().await.map_err(|_| VaultError::FileAccess)?;
    output
        .seek(SeekFrom::Start(0))
        .await
        .map_err(|_| VaultError::FileAccess)?;
    let padded_len = (len / BLOCK_BYTES as u64 + 1) * BLOCK_BYTES as u64;
    Ok((output, HEADER_BYTES as u64 + padded_len))
}

pub(super) async fn decrypt(
    mut source: File,
    len: u64,
    key: &SymmetricCryptoKey,
    destination: &Path,
) -> Result<tempfile::NamedTempFile, VaultError> {
    if !matches!(key, SymmetricCryptoKey::Aes256CbcHmacKey(_)) {
        return Err(VaultError::DecryptionFailed);
    }
    let ciphertext_len = len
        .checked_sub(HEADER_BYTES as u64)
        .filter(|len| *len > 0 && len.is_multiple_of(BLOCK_BYTES as u64))
        .ok_or(VaultError::DecryptionFailed)?;
    let key = key.to_encoded();
    let (encryption_key, mac_key) = key.as_ref().split_at(32);
    source
        .seek(SeekFrom::Start(0))
        .await
        .map_err(|_| VaultError::FileAccess)?;
    let mut header = [0; HEADER_BYTES];
    source
        .read_exact(&mut header)
        .await
        .map_err(|_| VaultError::DecryptionFailed)?;
    if header[0] != LEGACY_FORMAT {
        return Err(VaultError::DecryptionFailed);
    }
    let (iv, tag) = header[1..].split_at(BLOCK_BYTES);
    let mut mac =
        AttachmentMac::new_from_slice(mac_key).map_err(|_| VaultError::DecryptionFailed)?;
    mac.update(iv);
    let mut buffer = Zeroizing::new(vec![0; CHUNK_BYTES]);
    let mut remaining = ciphertext_len;
    while remaining != 0 {
        let count = remaining.min(CHUNK_BYTES as u64) as usize;
        source
            .read_exact(&mut buffer[..count])
            .await
            .map_err(|_| VaultError::DecryptionFailed)?;
        mac.update(&buffer[..count]);
        remaining -= count as u64;
    }
    mac.verify_slice(tag)
        .map_err(|_| VaultError::DecryptionFailed)?;

    // The encrypted spool is private and unchanged between passes. Verify
    // the entire payload before creating a file that may contain plaintext.
    source
        .seek(SeekFrom::Start(HEADER_BYTES as u64))
        .await
        .map_err(|_| VaultError::FileAccess)?;
    let temporary = download_temporary(destination)?;
    let mut output = File::from_std(temporary.reopen().map_err(|_| VaultError::FileAccess)?);
    let mut cipher = cbc::Decryptor::<Aes256>::new_from_slices(encryption_key, iv)
        .map_err(|_| VaultError::DecryptionFailed)?;
    let mut remaining = ciphertext_len;
    while remaining > CHUNK_BYTES as u64 {
        source
            .read_exact(&mut buffer)
            .await
            .map_err(|_| VaultError::DecryptionFailed)?;
        cipher.decrypt_blocks(Block::<Aes256>::slice_as_chunks_mut(&mut buffer).0);
        output
            .write_all(&buffer)
            .await
            .map_err(|_| VaultError::FileAccess)?;
        remaining -= CHUNK_BYTES as u64;
    }
    let tail = &mut buffer[..remaining as usize];
    source
        .read_exact(tail)
        .await
        .map_err(|_| VaultError::DecryptionFailed)?;
    let plaintext = cipher
        .decrypt_padded::<Pkcs7>(tail)
        .map_err(|_| VaultError::DecryptionFailed)?;
    output
        .write_all(plaintext)
        .await
        .map_err(|_| VaultError::FileAccess)?;
    output.flush().await.map_err(|_| VaultError::FileAccess)?;
    output
        .sync_all()
        .await
        .map_err(|_| VaultError::FileAccess)?;
    Ok(temporary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitwarden_crypto::{
        Decryptable, EncString, OctetStreamBytes, PrimitiveEncryptable, SymmetricKeyAlgorithm,
    };
    use bw_sdk_core::Client;

    #[tokio::test]
    async fn sdk_and_streaming_attachments_are_interoperable_across_chunk_boundaries() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        let sdk_wire = directory.path().join("sdk-wire");
        let client = Client::new(None);
        let store = client.internal.get_key_store();
        let key = SymmetricCryptoKey::make(SymmetricKeyAlgorithm::Aes256CbcHmac);

        for len in [
            0,
            BLOCK_BYTES,
            CHUNK_BYTES - 1,
            CHUNK_BYTES,
            CHUNK_BYTES * 2 + 7,
        ] {
            let payload: Vec<_> = (0..len).map(|i| (i % 251) as u8).collect();
            tokio::fs::write(&source, &payload).await.unwrap();
            let (mut actual, actual_len) =
                encrypt(File::open(&source).await.unwrap(), len as u64, &key)
                    .await
                    .unwrap();
            let mut wire = Vec::new();
            actual.read_to_end(&mut wire).await.unwrap();
            assert_eq!(wire.len() as u64, actual_len);

            let fixture = {
                let mut ctx = store.context();
                let slot = ctx.add_local_symmetric_key(key.clone());
                let plaintext: Vec<u8> = EncString::from_buffer(&wire)
                    .unwrap()
                    .decrypt(&mut ctx, slot)
                    .unwrap();
                assert_eq!(plaintext, payload);
                OctetStreamBytes::from(payload.as_slice())
                    .encrypt(&mut ctx, slot)
                    .unwrap()
                    .to_buffer()
                    .unwrap()
            };
            tokio::fs::write(&sdk_wire, &fixture).await.unwrap();
            let finished = decrypt(
                File::open(&sdk_wire).await.unwrap(),
                fixture.len() as u64,
                &key,
                &destination,
            )
            .await
            .unwrap();
            assert_eq!(tokio::fs::read(finished.path()).await.unwrap(), payload);
        }
    }
}
