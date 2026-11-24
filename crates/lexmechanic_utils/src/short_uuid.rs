use uuid::Uuid;
use std::collections::HashMap;

const DEFAULT_ALPHABET: &str = "23456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

pub struct ShortUuid {
    alphabet: Vec<char>,
    alphabet_len: u128,
    alphabet_map: HashMap<char, u128>,
    uuid_pad_len: usize,
}

impl Default for ShortUuid {
    fn default() -> Self {
        Self::new(None)
    }
}

impl ShortUuid {
    pub fn new(alphabet: Option<&str>) -> Self {
        let alpha_str = alphabet.unwrap_or(DEFAULT_ALPHABET);
        let mut chars: Vec<char> = alpha_str.chars().collect();
        // Python sorts it.
        chars.sort(); 
        
        let len = chars.len() as u128;
        let mut map = HashMap::new();
        for (i, &c) in chars.iter().enumerate() {
            map.insert(c, i as u128);
        }
        
        // uuid_pad_len = math.ceil(math.log(1 << 128, alphabet_len))
        // 1<<128 is 2^128. log(2^128, len) = 128 * log(2) / log(len) = 128 / log2(len)
        // Or loop to find coverage.
        // float usage is easiest.
        let pad_len = (128.0f64 * 2.0f64.ln() / (len as f64).ln()).ceil() as usize;

        ShortUuid {
            alphabet: chars,
            alphabet_len: len,
            alphabet_map: map,
            uuid_pad_len: pad_len,
        }
    }

    fn num_to_string(&self, mut number: u128, pad_to_length: Option<usize>) -> String {
        let mut ans = Vec::new();
        while number > 0 {
             let digit = (number % self.alphabet_len) as usize;
             number /= self.alphabet_len;
             ans.push(self.alphabet[digit]);
        }
        
        // Pad
        let target_len = pad_to_length.unwrap_or(self.uuid_pad_len);
        if target_len > ans.len() {
            let pad_char = self.alphabet[0];
            while ans.len() < target_len {
                ans.push(pad_char);
            }
        }
        
        // ans is little endian (least significant digit first)?
        // Python divmod loop produces least significant first. 
        // Python: ans.append(alphabet[digit]). 
        // Then ''.join(ans). 
        // Example: 10, base 10. 10%10 = 0. ans=[0]. num=1. 1%10=1. ans=[0,1]. join -> "01". 
        // Reversed string usually for numbers? 
        // Wait, Python `num_to_string` implementation:
        // ans.append(alphabet[digit]) ... returns ''.join(ans).
        // This produces "reversed" string (Least Significant Digit first).
        
        ans.iter().collect()
    }

    fn string_to_num(&self, s: &str) -> Option<u128> {
        let mut ans: u128 = 0;
        
        // Python: for char in reversed(string): ans = ans * len + map[char]
        // This treats the string as Big Endian? No.
        // "01". reversed -> '1', '0'.
        // '1': ans = 0*10 + 1 = 1.
        // '0': ans = 1*10 + 0 = 10.
        // So string "01" parses to 10.
        // Which matches num_to_string output "01" for 10.
        
        for c in s.chars().rev() {
             if let Some(&val) = self.alphabet_map.get(&c) {
                 // Check overflow?
                 // ans = ans * len + val
                 ans = ans.checked_mul(self.alphabet_len)?.checked_add(val)?;
             } else {
                 return None;
             }
        }
        Some(ans)
    }

    pub fn uuid4(&self, pad_to_length: Option<usize>) -> String {
        let u = Uuid::new_v4();
        self.num_to_string(u.as_u128(), pad_to_length)
    }
    
    pub fn uuid5(&self, namespace: &Uuid, name: &str, pad_to_length: Option<usize>) -> String {
        let u = Uuid::new_v5(namespace, name.as_bytes());
        self.num_to_string(u.as_u128(), pad_to_length)
    }
    
    pub fn decode(&self, encoded: &str) -> Option<Uuid> {
        let i = self.string_to_num(encoded)?;
        Some(Uuid::from_u128(i))
    }
}

// Global functions matching Python module
lazy_static::lazy_static! {
    static ref GLOBAL_INSTANCE: ShortUuid = ShortUuid::default();
}

pub fn uuid4() -> String {
    GLOBAL_INSTANCE.uuid4(None)
}

pub fn uuid5(namespace: &Uuid, name: &str) -> String {
    GLOBAL_INSTANCE.uuid5(namespace, name, None)
}

pub fn decode(encoded: &str) -> Option<Uuid> {
    GLOBAL_INSTANCE.decode(encoded)
}
