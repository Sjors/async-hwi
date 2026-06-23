use std::convert::{TryFrom, TryInto};
use std::io::Read;

use bitcoin::base64::Engine as _;
use bitcoin::consensus::encode::{deserialize, serialize, Decodable, Encodable, VarInt};
use bitcoin::psbt::{raw, Psbt};
use bitcoin::{
    absolute, transaction, Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid,
    Witness,
};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum PsbtFormat {
    V0,
    V2,
}

const PSBT_GLOBAL_UNSIGNED_TX: u8 = 0x00;
const PSBT_GLOBAL_TX_VERSION: u8 = 0x02;
const PSBT_GLOBAL_FALLBACK_LOCKTIME: u8 = 0x03;
const PSBT_GLOBAL_INPUT_COUNT: u8 = 0x04;
const PSBT_GLOBAL_OUTPUT_COUNT: u8 = 0x05;
const PSBT_GLOBAL_VERSION: u8 = 0xFB;

const PSBT_IN_PREVIOUS_TXID: u8 = 0x0E;
const PSBT_IN_OUTPUT_INDEX: u8 = 0x0F;
const PSBT_IN_SEQUENCE: u8 = 0x10;
const PSBT_IN_REQUIRED_TIME_LOCKTIME: u8 = 0x11;
const PSBT_IN_REQUIRED_HEIGHT_LOCKTIME: u8 = 0x12;

const PSBT_OUT_AMOUNT: u8 = 0x03;
const PSBT_OUT_SCRIPT: u8 = 0x04;

pub fn decode_base64(psbt_b64: &str) -> Result<(Psbt, PsbtFormat), String> {
    let raw = bitcoin::base64::engine::general_purpose::STANDARD
        .decode(psbt_b64.trim())
        .map_err(|e| format!("psbt base64 decode: {e}"))?;
    decode(&raw).map_err(|e| format!("psbt parse: {e}"))
}

pub fn encode_base64(psbt: &Psbt, format: PsbtFormat) -> String {
    let bytes = match format {
        PsbtFormat::V0 => psbt.serialize(),
        PsbtFormat::V2 => serialize_v2(psbt),
    };
    bitcoin::base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn decode(raw: &[u8]) -> Result<(Psbt, PsbtFormat), String> {
    let mut rest = strip_magic(raw)?;
    let global = read_map(&mut rest)?;
    let version = read_global_version(&global)?;
    if version != 2 {
        return Psbt::deserialize(raw)
            .map(|psbt| (psbt, PsbtFormat::V0))
            .map_err(|e| e.to_string());
    }

    let input_count = required_global_count(&global, PSBT_GLOBAL_INPUT_COUNT, "input count")?;
    let output_count = required_global_count(&global, PSBT_GLOBAL_OUTPUT_COUNT, "output count")?;
    let inputs = read_maps(&mut rest, input_count)?;
    let outputs = read_maps(&mut rest, output_count)?;
    if !rest.is_empty() {
        return Err("trailing bytes after PSBTv2 output maps".into());
    }

    let unsigned_tx = reconstruct_unsigned_tx(&global, &inputs, &outputs)?;
    let v0 = serialize_v0_view(&global, &inputs, &outputs, &unsigned_tx);
    Psbt::deserialize(&v0)
        .map(|psbt| (psbt, PsbtFormat::V2))
        .map_err(|e| e.to_string())
}

fn strip_magic(raw: &[u8]) -> Result<&[u8], String> {
    const MAGIC: &[u8] = b"psbt\xff";
    raw.strip_prefix(MAGIC)
        .ok_or_else(|| "invalid magic".to_string())
}

fn read_map(rest: &mut &[u8]) -> Result<Vec<raw::Pair>, String> {
    let mut pairs = Vec::new();
    loop {
        let VarInt(key_len) = VarInt::consensus_decode(rest).map_err(|e| e.to_string())?;
        if key_len == 0 {
            return Ok(pairs);
        }
        if key_len > usize::MAX as u64 {
            return Err("PSBT key too large".into());
        }
        let mut key_bytes = vec![0; key_len as usize];
        rest.read_exact(&mut key_bytes).map_err(|e| e.to_string())?;
        let (&type_value, key) = key_bytes
            .split_first()
            .ok_or_else(|| "empty PSBT key".to_string())?;
        let value: Vec<u8> = Decodable::consensus_decode(rest).map_err(|e| e.to_string())?;
        pairs.push(raw::Pair {
            key: raw::Key {
                type_value,
                key: key.to_vec(),
            },
            value,
        });
    }
}

fn read_maps(rest: &mut &[u8], count: usize) -> Result<Vec<Vec<raw::Pair>>, String> {
    (0..count).map(|_| read_map(rest)).collect()
}

fn read_global_version(global: &[raw::Pair]) -> Result<u32, String> {
    let mut version = None;
    for pair in global {
        if pair.key.type_value == PSBT_GLOBAL_VERSION {
            require_empty_key(pair, "global version")?;
            if version
                .replace(read_u32_le(&pair.value, "global version")?)
                .is_some()
            {
                return Err("duplicate global version".into());
            }
        }
    }
    Ok(version.unwrap_or(0))
}

fn required_global_count(
    global: &[raw::Pair],
    type_value: u8,
    name: &str,
) -> Result<usize, String> {
    let pair = global
        .iter()
        .find(|pair| pair.key.type_value == type_value)
        .ok_or_else(|| format!("missing PSBTv2 global {name}"))?;
    require_empty_key(pair, name)?;
    read_varint(&pair.value, name)
}

fn reconstruct_unsigned_tx(
    global: &[raw::Pair],
    inputs: &[Vec<raw::Pair>],
    outputs: &[Vec<raw::Pair>],
) -> Result<Transaction, String> {
    let tx_version = global
        .iter()
        .find(|pair| pair.key.type_value == PSBT_GLOBAL_TX_VERSION)
        .ok_or_else(|| "missing PSBTv2 global tx version".to_string())
        .and_then(|pair| {
            require_empty_key(pair, "tx version")?;
            read_i32_le(&pair.value, "tx version")
        })?;

    let fallback_locktime = global
        .iter()
        .find(|pair| pair.key.type_value == PSBT_GLOBAL_FALLBACK_LOCKTIME)
        .map(|pair| {
            require_empty_key(pair, "fallback locktime")?;
            read_u32_le(&pair.value, "fallback locktime")
        })
        .transpose()?;

    let parsed_inputs = inputs
        .iter()
        .map(|pairs| parse_v2_input(pairs))
        .collect::<Result<Vec<_>, _>>()?;
    let lock_time = compute_locktime(fallback_locktime, &parsed_inputs)?;
    let input = parsed_inputs
        .into_iter()
        .map(|input| TxIn {
            previous_output: OutPoint {
                txid: input.prev_txid,
                vout: input.vout,
            },
            script_sig: ScriptBuf::new(),
            sequence: input.sequence,
            witness: Witness::new(),
        })
        .collect();
    let output = outputs
        .iter()
        .map(|pairs| parse_v2_output(pairs))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Transaction {
        version: transaction::Version(tx_version),
        lock_time: absolute::LockTime::from_consensus(lock_time),
        input,
        output,
    })
}

struct ParsedInput {
    prev_txid: Txid,
    vout: u32,
    sequence: Sequence,
    time_locktime: Option<u32>,
    height_locktime: Option<u32>,
}

fn parse_v2_input(pairs: &[raw::Pair]) -> Result<ParsedInput, String> {
    let mut prev_txid = None;
    let mut vout = None;
    let mut sequence = None;
    let mut time_locktime = None;
    let mut height_locktime = None;

    for pair in pairs {
        match pair.key.type_value {
            PSBT_IN_PREVIOUS_TXID => {
                require_empty_key(pair, "previous txid")?;
                if prev_txid
                    .replace(deserialize(&pair.value).map_err(|e| e.to_string())?)
                    .is_some()
                {
                    return Err("duplicate previous txid".into());
                }
            }
            PSBT_IN_OUTPUT_INDEX => {
                require_empty_key(pair, "output index")?;
                if vout
                    .replace(read_u32_le(&pair.value, "output index")?)
                    .is_some()
                {
                    return Err("duplicate output index".into());
                }
            }
            PSBT_IN_SEQUENCE => {
                require_empty_key(pair, "sequence")?;
                if sequence
                    .replace(Sequence::from_consensus(read_u32_le(
                        &pair.value,
                        "sequence",
                    )?))
                    .is_some()
                {
                    return Err("duplicate sequence".into());
                }
            }
            PSBT_IN_REQUIRED_TIME_LOCKTIME => {
                require_empty_key(pair, "required time locktime")?;
                if time_locktime
                    .replace(read_u32_le(&pair.value, "required time locktime")?)
                    .is_some()
                {
                    return Err("duplicate required time locktime".into());
                }
            }
            PSBT_IN_REQUIRED_HEIGHT_LOCKTIME => {
                require_empty_key(pair, "required height locktime")?;
                if height_locktime
                    .replace(read_u32_le(&pair.value, "required height locktime")?)
                    .is_some()
                {
                    return Err("duplicate required height locktime".into());
                }
            }
            _ => {}
        }
    }

    Ok(ParsedInput {
        prev_txid: prev_txid.ok_or_else(|| "missing PSBTv2 input previous txid".to_string())?,
        vout: vout.ok_or_else(|| "missing PSBTv2 input output index".to_string())?,
        sequence: sequence.unwrap_or(Sequence::MAX),
        time_locktime,
        height_locktime,
    })
}

fn parse_v2_output(pairs: &[raw::Pair]) -> Result<TxOut, String> {
    let mut value = None;
    let mut script_pubkey = None;
    for pair in pairs {
        match pair.key.type_value {
            PSBT_OUT_AMOUNT => {
                require_empty_key(pair, "output amount")?;
                if value
                    .replace(Amount::from_sat(read_u64_le(&pair.value, "output amount")?))
                    .is_some()
                {
                    return Err("duplicate output amount".into());
                }
            }
            PSBT_OUT_SCRIPT => {
                require_empty_key(pair, "output script")?;
                if script_pubkey
                    .replace(ScriptBuf::from(pair.value.clone()))
                    .is_some()
                {
                    return Err("duplicate output script".into());
                }
            }
            _ => {}
        }
    }
    Ok(TxOut {
        value: value.ok_or_else(|| "missing PSBTv2 output amount".to_string())?,
        script_pubkey: script_pubkey.ok_or_else(|| "missing PSBTv2 output script".to_string())?,
    })
}

fn compute_locktime(fallback: Option<u32>, inputs: &[ParsedInput]) -> Result<u32, String> {
    let mut time_lock = Some(0);
    let mut height_lock = Some(0);
    for input in inputs {
        match (input.time_locktime, input.height_locktime) {
            (Some(time), None) => {
                height_lock = None;
                let current = time_lock
                    .as_mut()
                    .ok_or_else(|| "incompatible PSBTv2 input locktime types".to_string())?;
                *current = (*current).max(time);
            }
            (None, Some(height)) => {
                time_lock = None;
                let current = height_lock
                    .as_mut()
                    .ok_or_else(|| "incompatible PSBTv2 input locktime types".to_string())?;
                *current = (*current).max(height);
            }
            (Some(_), Some(_)) => {
                return Err("input has both time and height locktime".into());
            }
            (None, None) => {}
        }
    }
    if let Some(height) = height_lock {
        if height > 0 {
            return Ok(height);
        }
    }
    if let Some(time) = time_lock {
        if time > 0 {
            return Ok(time);
        }
    }
    Ok(fallback.unwrap_or(0))
}

fn serialize_v0_view(
    global: &[raw::Pair],
    inputs: &[Vec<raw::Pair>],
    outputs: &[Vec<raw::Pair>],
    unsigned_tx: &Transaction,
) -> Vec<u8> {
    let mut out = Vec::from(b"psbt\xff");
    serialize_pair(
        &mut out,
        &raw::Pair {
            key: raw::Key {
                type_value: PSBT_GLOBAL_UNSIGNED_TX,
                key: vec![],
            },
            value: serialize(unsigned_tx),
        },
    );
    for pair in global {
        if !matches!(
            pair.key.type_value,
            PSBT_GLOBAL_TX_VERSION
                | PSBT_GLOBAL_FALLBACK_LOCKTIME
                | PSBT_GLOBAL_INPUT_COUNT
                | PSBT_GLOBAL_OUTPUT_COUNT
                | PSBT_GLOBAL_VERSION
        ) {
            serialize_pair(&mut out, pair);
        }
    }
    out.push(0);
    for input in inputs {
        for pair in input {
            if !matches!(
                pair.key.type_value,
                PSBT_IN_PREVIOUS_TXID
                    | PSBT_IN_OUTPUT_INDEX
                    | PSBT_IN_SEQUENCE
                    | PSBT_IN_REQUIRED_TIME_LOCKTIME
                    | PSBT_IN_REQUIRED_HEIGHT_LOCKTIME
            ) {
                serialize_pair(&mut out, pair);
            }
        }
        out.push(0);
    }
    for output in outputs {
        for pair in output {
            if !matches!(pair.key.type_value, PSBT_OUT_AMOUNT | PSBT_OUT_SCRIPT) {
                serialize_pair(&mut out, pair);
            }
        }
        out.push(0);
    }
    out
}

fn serialize_v2(psbt: &Psbt) -> Vec<u8> {
    let mut out = Vec::from(b"psbt\xff");
    serialize_map(
        &mut out,
        ledger_bitcoin_client::psbt::get_v2_global_pairs(psbt),
    );
    for (input, txin) in psbt.inputs.iter().zip(&psbt.unsigned_tx.input) {
        serialize_map(
            &mut out,
            ledger_bitcoin_client::psbt::get_v2_input_pairs(input, txin),
        );
    }
    for (output, txout) in psbt.outputs.iter().zip(&psbt.unsigned_tx.output) {
        serialize_map(
            &mut out,
            ledger_bitcoin_client::psbt::get_v2_output_pairs(output, txout),
        );
    }
    out
}

fn serialize_map(out: &mut Vec<u8>, pairs: Vec<raw::Pair>) {
    for pair in pairs {
        serialize_pair(out, &pair);
    }
    out.push(0);
}

fn serialize_pair(out: &mut Vec<u8>, pair: &raw::Pair) {
    VarInt((pair.key.key.len() + 1) as u64)
        .consensus_encode(out)
        .expect("Vec writer cannot fail");
    pair.key
        .type_value
        .consensus_encode(out)
        .expect("Vec writer cannot fail");
    out.extend_from_slice(&pair.key.key);
    pair.value
        .consensus_encode(out)
        .expect("Vec writer cannot fail");
}

fn require_empty_key(pair: &raw::Pair, name: &str) -> Result<(), String> {
    if pair.key.key.is_empty() {
        Ok(())
    } else {
        Err(format!("{name} key data must be empty"))
    }
}

fn read_varint(value: &[u8], name: &str) -> Result<usize, String> {
    let mut rest = value;
    let VarInt(n) = VarInt::consensus_decode(&mut rest).map_err(|e| e.to_string())?;
    if !rest.is_empty() {
        return Err(format!("{name} has trailing bytes"));
    }
    usize::try_from(n).map_err(|_| format!("{name} too large"))
}

fn read_i32_le(value: &[u8], name: &str) -> Result<i32, String> {
    Ok(i32::from_le_bytes(read_array(value, name)?))
}

fn read_u32_le(value: &[u8], name: &str) -> Result<u32, String> {
    Ok(u32::from_le_bytes(read_array(value, name)?))
}

fn read_u64_le(value: &[u8], name: &str) -> Result<u64, String> {
    Ok(u64::from_le_bytes(read_array(value, name)?))
}

fn read_array<const N: usize>(value: &[u8], name: &str) -> Result<[u8; N], String> {
    value
        .try_into()
        .map_err(|_| format!("{name} must be {N} bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;

    #[test]
    fn psbtv2_roundtrips_through_v0_model() {
        let tx = Transaction {
            version: transaction::Version(2),
            lock_time: absolute::LockTime::from_consensus(42),
            input: vec![TxIn {
                previous_output: OutPoint {
                    txid: Txid::from_slice(&[3; 32]).unwrap(),
                    vout: 7,
                },
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_LOCKTIME_NO_RBF,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: ScriptBuf::new(),
            }],
        };
        let mut psbt = Psbt::from_unsigned_tx(tx).unwrap();
        psbt.inputs[0].witness_utxo = Some(psbt.unsigned_tx.output[0].clone());

        let encoded_v2 = serialize_v2(&psbt);
        let (decoded, format) = decode(&encoded_v2).unwrap();

        assert_eq!(format, PsbtFormat::V2);
        assert_eq!(decoded, psbt);
        assert!(decoded.unknown.is_empty());
        assert!(decoded.inputs[0].unknown.is_empty());
        assert!(decoded.outputs[0].unknown.is_empty());
    }
}
