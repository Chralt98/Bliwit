use std::env;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use futarchy_fixed::{lmsr_cost, FixedU64x64, COMPOSED_COST_MAX_ULP, PRIMITIVE_MAX_ULP};

const SWEEP_SCHEMA: &str = "bleavit.reference-model.v3";
const FULL_SWEEP_POINTS: u128 = 10_000_000;
const ONE_RAW: u128 = 1u128 << 64;

#[derive(Debug)]
struct Shard {
    file: String,
    rows: u128,
    sha256: String,
}

#[derive(Debug)]
struct Row<'a> {
    function: &'a str,
    input: Option<u128>,
    q_l: Option<u128>,
    q_s: Option<u128>,
    b: Option<u128>,
    expected: u128,
}

fn json_value_start<'a>(text: &'a str, key: &str) -> Result<&'a str, String> {
    let quoted_key = format!("\"{key}\"");
    let key_start = text
        .find(&quoted_key)
        .ok_or_else(|| format!("missing JSON key {key}"))?;
    let after_key = &text[key_start + quoted_key.len()..];
    let colon = after_key
        .find(':')
        .ok_or_else(|| format!("missing colon after JSON key {key}"))?;
    Ok(after_key[colon + 1..].trim_start())
}

fn json_string(text: &str, key: &str) -> Result<String, String> {
    let value = json_value_start(text, key)?;
    let value = value
        .strip_prefix('"')
        .ok_or_else(|| format!("JSON key {key} is not a string"))?;
    let end = value
        .find('"')
        .ok_or_else(|| format!("unterminated string for JSON key {key}"))?;
    Ok(value[..end].to_owned())
}

fn json_u128(text: &str, key: &str) -> Result<u128, String> {
    let value = json_value_start(text, key)?;
    let digits = value
        .as_bytes()
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 {
        return Err(format!("JSON key {key} is not an unsigned integer"));
    }
    value[..digits]
        .parse::<u128>()
        .map_err(|error| format!("invalid integer for JSON key {key}: {error}"))
}

fn parse_shards(manifest: &str) -> Result<Vec<Shard>, String> {
    let value = json_value_start(manifest, "shards")?;
    let array = value
        .strip_prefix('[')
        .ok_or_else(|| "manifest shards value is not an array".to_owned())?;
    let mut remaining = array;
    let mut shards = Vec::new();
    loop {
        remaining = remaining.trim_start_matches(|character: char| {
            character.is_ascii_whitespace() || character == ','
        });
        if remaining.starts_with(']') {
            break;
        }
        let object = remaining
            .strip_prefix('{')
            .ok_or_else(|| "manifest shard entry is not an object".to_owned())?;
        let end = object
            .find('}')
            .ok_or_else(|| "unterminated manifest shard entry".to_owned())?;
        let entry = &object[..end];
        shards.push(Shard {
            file: json_string(entry, "file")?,
            rows: json_u128(entry, "rows")?,
            sha256: json_string(entry, "sha256")?,
        });
        remaining = &object[end + 1..];
    }
    if shards.is_empty() {
        return Err("manifest contains no shards".to_owned());
    }
    Ok(shards)
}

fn parse_row(line: &str) -> Result<Row<'_>, String> {
    let line = line.trim().strip_suffix(',').unwrap_or(line.trim());
    let after_function = line
        .strip_prefix("{\"f\":\"")
        .ok_or_else(|| "row does not start with the fixed-order f key".to_owned())?;
    let (function, values) = after_function
        .split_once("\",")
        .ok_or_else(|| "row is missing fields after f".to_owned())?;
    if function == "cost" {
        let values = values
            .strip_prefix("\"q_l\":")
            .ok_or_else(|| "cost row is missing fixed-order q_l key".to_owned())?;
        let (q_l, values) = values
            .split_once(",\"q_s\":")
            .ok_or_else(|| "cost row is missing fixed-order q_s key".to_owned())?;
        let (q_s, values) = values
            .split_once(",\"b\":")
            .ok_or_else(|| "cost row is missing fixed-order b key".to_owned())?;
        let (b, output) = values
            .split_once(",\"out\":")
            .ok_or_else(|| "cost row is missing fixed-order out key".to_owned())?;
        return Ok(Row {
            function,
            input: None,
            q_l: Some(json_integer(q_l, "q_l")?),
            q_s: Some(json_integer(q_s, "q_s")?),
            b: Some(json_integer(b, "b")?),
            expected: json_integer(
                output
                    .strip_suffix('}')
                    .ok_or_else(|| "row is not a closed JSON object".to_owned())?,
                "out",
            )?,
        });
    }
    let values = values
        .strip_prefix("\"in\":")
        .ok_or_else(|| "primitive row is missing fixed-order in key".to_owned())?;
    let (input, output) = values
        .split_once(",\"out\":")
        .ok_or_else(|| "primitive row is missing fixed-order out key".to_owned())?;
    Ok(Row {
        function,
        input: Some(json_integer(input, "in")?),
        q_l: None,
        q_s: None,
        b: None,
        expected: json_integer(
            output
                .strip_suffix('}')
                .ok_or_else(|| "row is not a closed JSON object".to_owned())?,
            "out",
        )?,
    })
}

fn json_integer(value: &str, name: &str) -> Result<u128, String> {
    value
        .parse::<u128>()
        .map_err(|error| format!("invalid row {name}: {error}"))
}

fn evaluate(row: &Row<'_>) -> Result<u128, String> {
    let result = match row.function {
        "exp2" => FixedU64x64::from_raw(row.input.ok_or("exp2 input missing")?).exp2(),
        "log2" => FixedU64x64::from_raw(row.input.ok_or("log2 input missing")?).log2(),
        "ln" => FixedU64x64::from_raw(row.input.ok_or("ln input missing")?).ln(),
        "cost" => lmsr_cost(
            FixedU64x64::from_raw(row.q_l.ok_or("cost q_l missing")?),
            FixedU64x64::from_raw(row.q_s.ok_or("cost q_s missing")?),
            FixedU64x64::from_raw(row.b.ok_or("cost b missing")?),
        ),
        other => return Err(format!("unknown sweep function {other}")),
    };
    result
        .map(FixedU64x64::raw)
        .map_err(|error| format!("{} row failed: {error:?}", row.function))
}

struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buffer_len: usize,
    bytes: u64,
}

impl Sha256 {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buffer: [0; 64],
            buffer_len: 0,
            bytes: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.bytes = self
            .bytes
            .checked_add(u64::try_from(data.len()).expect("input length fits u64"))
            .expect("SHA-256 input length fits u64");
        if self.buffer_len != 0 {
            let take = (64 - self.buffer_len).min(data.len());
            self.buffer[self.buffer_len..self.buffer_len + take].copy_from_slice(&data[..take]);
            self.buffer_len += take;
            data = &data[take..];
            if self.buffer_len == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.buffer_len = 0;
            }
        }
        while data.len() >= 64 {
            self.compress(&data[..64]);
            data = &data[64..];
        }
        self.buffer[..data.len()].copy_from_slice(data);
        self.buffer_len = data.len();
    }

    fn compress(&mut self, block: &[u8]) {
        let mut words = [0u32; 64];
        for (index, word) in words[..16].iter_mut().enumerate() {
            let start = index * 4;
            *word = u32::from_be_bytes(
                block[start..start + 4]
                    .try_into()
                    .expect("SHA-256 block word is four bytes"),
            );
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for (&word, &constant) in words.iter().zip(Self::K.iter()) {
            let sum1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temporary1 = h
                .wrapping_add(sum1)
                .wrapping_add(choose)
                .wrapping_add(constant)
                .wrapping_add(word);
            let sum0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temporary2 = sum0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temporary1);
            d = c;
            c = b;
            b = a;
            a = temporary1.wrapping_add(temporary2);
        }
        for (state, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *state = state.wrapping_add(value);
        }
    }

    fn finish(mut self) -> [u8; 32] {
        let bit_length = self
            .bytes
            .checked_mul(8)
            .expect("SHA-256 bit length fits u64");
        self.buffer[self.buffer_len] = 0x80;
        self.buffer_len += 1;
        if self.buffer_len > 56 {
            self.buffer[self.buffer_len..].fill(0);
            let block = self.buffer;
            self.compress(&block);
            self.buffer = [0; 64];
        } else {
            self.buffer[self.buffer_len..56].fill(0);
        }
        self.buffer[56..].copy_from_slice(&bit_length.to_be_bytes());
        let block = self.buffer;
        self.compress(&block);

        let mut output = [0u8; 32];
        for (chunk, word) in output.chunks_exact_mut(4).zip(self.state) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        output
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest.finish() {
        write!(&mut hex, "{byte:02x}").expect("writing to String cannot fail");
    }
    hex
}

fn check_shard_path(file: &str, shard_index: usize) -> Result<(), String> {
    let expected = format!("shards/sweep-{shard_index:03}.json");
    if file != expected {
        return Err(format!(
            "shard {shard_index} file must be {expected}, found {file}"
        ));
    }
    if Path::new(file)
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!("unsafe shard path {file}"));
    }
    Ok(())
}

#[test]
#[ignore = "release-only ≥10^7-point MPFR-256 differential sweep"]
fn reference_model_transcendental_sweep_matches() {
    let sweep_dir = env::var_os("BLEAVIT_SWEEP_DIR")
        .map(PathBuf::from)
        .expect("BLEAVIT_SWEEP_DIR must point to a generated sweep directory");
    // Cargo runs test binaries from the package root, not the workspace root;
    // the CI workflows pass workspace-relative paths, so resolve them there.
    let sweep_dir = if sweep_dir.is_absolute() {
        sweep_dir
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(sweep_dir)
    };
    let manifest_path = sweep_dir.join("sweep-manifest.json");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", manifest_path.display()));
    assert_eq!(json_string(&manifest, "schema").unwrap(), SWEEP_SCHEMA);
    assert_eq!(
        json_string(&manifest, "kind").unwrap(),
        "transcendental-sweep"
    );
    assert_eq!(
        json_string(&manifest, "exp2_relative_bound").unwrap(),
        "2**-63"
    );
    assert_eq!(
        json_u128(&manifest, "primitive_abs_ulp_bound").unwrap(),
        u128::from(PRIMITIVE_MAX_ULP)
    );
    assert_eq!(
        json_u128(&manifest, "composed_cost_abs_ulp_bound").unwrap(),
        u128::from(COMPOSED_COST_MAX_ULP)
    );
    let declared_points = json_u128(&manifest, "points").unwrap();
    if env::var("BLEAVIT_SWEEP_REQUIRE_FULL").as_deref() == Ok("1") {
        assert!(
            declared_points >= FULL_SWEEP_POINTS,
            "release mode requires ≥{FULL_SWEEP_POINTS} points; manifest has {declared_points}"
        );
    }
    let shards = parse_shards(&manifest).unwrap();
    let mut checked = 0u128;
    let mut manifest_rows = 0u128;
    let mut worst_exp2_ulp = 0u128;
    let mut worst_log2_ulp = 0u128;
    let mut worst_ln_ulp = 0u128;
    let mut worst_cost_ulp = 0u128;

    let mut sha_test = Sha256::new();
    sha_test.update(b"abc");
    let sha_test_hex = sha_test
        .finish()
        .iter()
        .fold(String::new(), |mut output, byte| {
            write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
            output
        });
    assert_eq!(
        sha_test_hex,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );

    for (shard_index, shard) in shards.iter().enumerate() {
        check_shard_path(&shard.file, shard_index).unwrap();
        let shard_path = sweep_dir.join(&shard.file);
        let shard_bytes = std::fs::read(&shard_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", shard_path.display()));
        assert_eq!(
            sha256_hex(&shard_bytes),
            shard.sha256,
            "content hash mismatch for {}",
            shard.file
        );
        let shard_text = std::str::from_utf8(&shard_bytes)
            .unwrap_or_else(|error| panic!("{} is not UTF-8: {error}", shard.file));
        let mut lines = shard_text.lines();
        let header = lines.next().expect("shard must contain a header");
        assert_eq!(
            header,
            format!("{{\"schema\":\"{SWEEP_SCHEMA}\",\"shard\":{shard_index},\"rows\":["),
            "invalid header for {}",
            shard.file
        );
        let mut shard_rows = 0u128;
        let mut saw_footer = false;
        for (line_index, line) in lines.enumerate() {
            if line == "]}" {
                assert!(!saw_footer, "duplicate footer in {}", shard.file);
                saw_footer = true;
                continue;
            }
            assert!(!saw_footer, "data follows footer in {}", shard.file);
            let row = parse_row(line)
                .unwrap_or_else(|error| panic!("{} line {}: {error}", shard.file, line_index + 2));
            let actual = evaluate(&row)
                .unwrap_or_else(|error| panic!("{} line {}: {error}", shard.file, line_index + 2));
            let ulp = actual.abs_diff(row.expected);
            match row.function {
                "exp2" => {
                    let tolerance = row.expected >> 63;
                    let input = row.input.expect("exp2 input exists");
                    assert!(
                        ulp <= tolerance,
                        "exp2({}) relative error {ulp} > {tolerance} (2^-63)",
                        input
                    );
                    worst_exp2_ulp = worst_exp2_ulp.max(ulp);
                }
                "log2" => {
                    let input = row.input.expect("log2 input exists");
                    assert!(
                        ulp <= u128::from(PRIMITIVE_MAX_ULP),
                        "log2({}) absolute error {ulp} > {PRIMITIVE_MAX_ULP} ulp",
                        input
                    );
                    worst_log2_ulp = worst_log2_ulp.max(ulp);
                }
                "ln" => {
                    let input = row.input.expect("ln input exists");
                    assert!(
                        ulp <= u128::from(PRIMITIVE_MAX_ULP),
                        "ln({}) absolute error {ulp} > {PRIMITIVE_MAX_ULP} ulp",
                        input
                    );
                    worst_ln_ulp = worst_ln_ulp.max(ulp);
                }
                "cost" => {
                    // The in-crate corpus scales the composed allowance by b.
                    // Express that same comparison as absolute ulps of the
                    // normalized cost kernel so the asserted bound remains 8.
                    // Deliberately stricter than the in-crate trade-path check:
                    // that one adds a USDC base unit for payment rounding, a
                    // different quantity from the pure composed cost certified
                    // here against the 04 §4 bound.
                    let b_units = row.b.expect("cost b exists") / ONE_RAW;
                    let normalized_ulp = ulp.div_ceil(b_units);
                    assert!(
                        normalized_ulp <= u128::from(COMPOSED_COST_MAX_ULP),
                        "cost({}, {}, {}) normalized absolute error {normalized_ulp} > {COMPOSED_COST_MAX_ULP} ulp",
                        row.q_l.expect("cost q_l exists"),
                        row.q_s.expect("cost q_s exists"),
                        row.b.expect("cost b exists"),
                    );
                    worst_cost_ulp = worst_cost_ulp.max(normalized_ulp);
                }
                _ => unreachable!("evaluate rejects unknown functions"),
            }
            shard_rows += 1;
            checked += 1;
        }
        assert!(saw_footer, "missing footer in {}", shard.file);
        assert_eq!(
            shard_rows, shard.rows,
            "row count mismatch for {}",
            shard.file
        );
        manifest_rows += shard.rows;
    }

    assert_eq!(manifest_rows, declared_points, "manifest shard accounting");
    assert_eq!(checked, declared_points, "checked point accounting");
    println!(
        "Bleavit sweep: points checked={checked}; worst exp2 full-range ulp={worst_exp2_ulp}; \
         worst log2 ulp={worst_log2_ulp}; worst ln ulp={worst_ln_ulp}; \
         worst cost ulp={worst_cost_ulp}"
    );
}
