use rsa::pkcs8::EncodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey};
use sha1::{Digest, Sha1};

const RSA_BITS: usize = 1024;
const SERVER_ID: &[u8] = b"";

pub struct ServerKey {
    pub private_key: RsaPrivateKey,
    pub der_public_key: Vec<u8>,
}

impl ServerKey {
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let private_key =
            RsaPrivateKey::new(&mut rng, RSA_BITS).expect("failed to generate RSA keypair");
        let der_public_key = private_key
            .to_public_key()
            .to_public_key_der()
            .expect("failed to encode public key")
            .to_vec();
        Self {
            private_key,
            der_public_key,
        }
    }

    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>, rsa::Error> {
        self.private_key.decrypt(Pkcs1v15Encrypt, data)
    }
}

pub fn minecraft_hex_digest(shared_secret: &[u8], public_key_der: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(SERVER_ID);
    hasher.update(shared_secret);
    hasher.update(public_key_der);
    let hash: [u8; 20] = hasher.finalize().into();

    let negative = (hash[0] & 0x80) != 0;
    if negative {
        let mut carry = true;
        let mut negated = [0u8; 20];
        for i in (0..20).rev() {
            let (val, c) = (!hash[i]).overflowing_add(u8::from(carry));
            negated[i] = val;
            carry = c;
        }
        let hex: String = negated.iter().map(|b| format!("{b:02x}")).collect();
        format!("-{}", hex.trim_start_matches('0'))
    } else {
        let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        hex.trim_start_matches('0').to_string()
    }
}

pub struct CipherState {
    encrypt_state: [u8; 16],
    key: [u8; 16],
}

impl CipherState {
    pub fn new(shared_secret: &[u8; 16]) -> Self {
        Self {
            encrypt_state: *shared_secret,
            key: *shared_secret,
        }
    }

    pub fn encrypt(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            let encrypted = aes_encrypt_block(&self.key, &self.encrypt_state)[0] ^ *byte;
            shift_iv(&mut self.encrypt_state, encrypted);
            *byte = encrypted;
        }
    }
}

fn aes_encrypt_block(key: &[u8; 16], input: &[u8; 16]) -> [u8; 16] {
    use aes::Aes128;
    use aes::cipher::{BlockEncrypt, KeyInit};

    let cipher = Aes128::new(key.into());
    let mut block = aes::Block::from(*input);
    cipher.encrypt_block(&mut block);
    block.into()
}

fn shift_iv(iv: &mut [u8; 16], new_byte: u8) {
    iv.copy_within(1.., 0);
    iv[15] = new_byte;
}
