use crate::ports::ContentHasher;

pub struct Blake3ContentHasher;

impl ContentHasher for Blake3ContentHasher {
    fn hash(&self, bytes: &[u8]) -> [u8; 32] {
        *blake3::hash(bytes).as_bytes()
    }
}
