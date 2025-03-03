//! Core AEAD cipher implementation for (X)ChaCha20Poly1305.

// TODO(tarcieri): make this reusable for (X)Salsa20Poly1305

use aead::generic_array::GenericArray;
use aead::{Error, Payload};
use alloc::vec::Vec;
use chacha20::stream_cipher::{SyncStreamCipher, SyncStreamCipherSeek};
use core::convert::TryInto;
use poly1305::{Poly1305, Tag};
use zeroize::Zeroizing;

/// ChaCha20Poly1305 instantiated with a particular nonce
pub(crate) struct Cipher<C>
where
    C: SyncStreamCipher + SyncStreamCipherSeek,
{
    cipher: C,
    mac: Poly1305,
}

impl<C> Cipher<C>
where
    C: SyncStreamCipher + SyncStreamCipherSeek,
{
    /// Instantiate the underlying cipher with a particular nonce
    pub(crate) fn new(mut cipher: C) -> Self {
        // Derive Poly1305 key from the first 32-bytes of the ChaCha20 keystream
        let mut mac_key = Zeroizing::new([0u8; poly1305::KEY_SIZE]);
        cipher.apply_keystream(&mut *mac_key);
        let mac = Poly1305::new(&mac_key);

        // Set ChaCha20 counter to 1
        cipher.seek(chacha20::BLOCK_SIZE as u64);

        Self { cipher, mac }
    }

    /// Encrypt the given message, allocating a vector for the resulting ciphertext
    pub(crate) fn encrypt(self, payload: Payload) -> Result<Vec<u8>, Error> {
        let mut buffer = Vec::with_capacity(payload.msg.len() + poly1305::BLOCK_SIZE);
        buffer.extend_from_slice(payload.msg);

        let tag = self.encrypt_in_place(&mut buffer, payload.aad)?;
        buffer.extend_from_slice(tag.code().as_slice());
        Ok(buffer)
    }

    /// Encrypt the given message in-place, returning the authentication tag
    pub(crate) fn encrypt_in_place(
        mut self,
        buffer: &mut [u8],
        associated_data: &[u8],
    ) -> Result<Tag, Error> {
        if buffer.len() / chacha20::BLOCK_SIZE >= chacha20::MAX_BLOCKS {
            return Err(Error);
        }

        self.mac.input_padded(associated_data);
        self.cipher.apply_keystream(buffer);
        self.mac.input_padded(buffer);
        self.authenticate_lengths(associated_data, buffer)?;
        Ok(self.mac.result())
    }

    /// Decrypt the given message, allocating a vector for the resulting plaintext
    pub(crate) fn decrypt(self, payload: Payload) -> Result<Vec<u8>, Error> {
        if payload.msg.len() < poly1305::BLOCK_SIZE {
            return Err(Error);
        }

        let tag_start = payload.msg.len() - poly1305::BLOCK_SIZE;
        let mut buffer = Vec::from(&payload.msg[..tag_start]);
        let tag: [u8; poly1305::BLOCK_SIZE] = payload.msg[tag_start..].try_into().unwrap();
        self.decrypt_in_place(&mut buffer, payload.aad, &tag)?;

        Ok(buffer)
    }

    /// Decrypt the given message, first authenticating ciphertext integrity
    /// and returning an error if it's been tampered with.
    pub(crate) fn decrypt_in_place(
        mut self,
        buffer: &mut [u8],
        associated_data: &[u8],
        tag: &[u8; poly1305::BLOCK_SIZE],
    ) -> Result<(), Error> {
        if buffer.len() / chacha20::BLOCK_SIZE >= chacha20::MAX_BLOCKS {
            return Err(Error);
        }

        self.mac.input_padded(associated_data);
        self.mac.input_padded(buffer);
        self.authenticate_lengths(associated_data, buffer)?;

        // This performs a constant-time comparison using the `subtle` crate
        if self.mac.result() == Tag::new(*GenericArray::from_slice(tag)) {
            self.cipher.apply_keystream(buffer);
            Ok(())
        } else {
            Err(Error)
        }
    }

    /// Authenticate the lengths of the associated data and message
    fn authenticate_lengths(&mut self, associated_data: &[u8], buffer: &[u8]) -> Result<(), Error> {
        let associated_data_len: u64 = associated_data.len().try_into().map_err(|_| Error)?;
        let buffer_len: u64 = buffer.len().try_into().map_err(|_| Error)?;
        self.mac.input(&associated_data_len.to_le_bytes());
        self.mac.input(&buffer_len.to_le_bytes());
        Ok(())
    }
}
