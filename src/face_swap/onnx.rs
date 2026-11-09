//! Reads one constant tensor out of an ONNX file without loading the
//! model: the swapper keeps the matrix that turns an identity embedding
//! into its own latent ("emap", 512 × 512) as a graph initializer, which
//! ONNX Runtime doesn't hand out. The file is a protobuf; this walks its
//! fields, skipping everything (the big weights included) but the
//! initializers of the right size.
//!
//! Wire format: ModelProto.graph = 7; GraphProto.initializer = 5;
//! TensorProto: dims = 1, data_type = 2 (1 float, 10 float16),
//! float_data = 4, name = 8, raw_data = 9.

use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;

use super::error::{FaceSwapError, Result};

const FLOAT: u64 = 1;
const FLOAT16: u64 = 10;

/// A float tensor read from the file.
#[derive(Debug, Clone, PartialEq)]
pub struct Initializer {
    pub name: String,
    pub dims: Vec<i64>,
    pub data: Vec<f32>,
}

/// The last float initializer of `path` whose shape is `dims`.
pub fn find_initializer(path: &Path, dims: &[i64]) -> Result<Initializer> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut r = BufReader::with_capacity(1 << 16, file);
    let elems: i64 = dims.iter().product();
    // A tensor of that size in f16 or f32 plus a little framing.
    let (lo, hi) = (elems as u64 * 2, elems as u64 * 4 + 4096);
    let mut found = None;
    while r.stream_position()? < len {
        let (field, wire) = read_key(&mut r)?;
        if field == 7 && wire == 2 {
            let graph_len = read_varint(&mut r)?;
            let graph_end = r.stream_position()? + graph_len;
            while r.stream_position()? < graph_end {
                let (gf, gw) = read_key(&mut r)?;
                if gf == 5 && gw == 2 {
                    let n = read_varint(&mut r)?;
                    if (lo..=hi).contains(&n) {
                        let mut buf = vec![0u8; n as usize];
                        r.read_exact(&mut buf)?;
                        if let Some(t) = parse_tensor(&buf)
                            && t.dims == dims
                        {
                            found = Some(t);
                        }
                    } else {
                        r.seek_relative(n as i64)?;
                    }
                } else {
                    skip(&mut r, gw)?;
                }
            }
        } else {
            skip(&mut r, wire)?;
        }
    }
    found.ok_or_else(|| FaceSwapError::ModelShape(format!("no {dims:?} matrix in the swap model")))
}

fn read_varint(r: &mut impl Read) -> Result<u64> {
    let mut out = 0u64;
    for shift in (0..64).step_by(7) {
        let mut b = [0u8; 1];
        r.read_exact(&mut b)?;
        out |= ((b[0] & 0x7f) as u64) << shift;
        if b[0] & 0x80 == 0 {
            return Ok(out);
        }
    }
    Err(FaceSwapError::ModelShape("bad protobuf varint".into()))
}

fn read_key(r: &mut impl Read) -> Result<(u64, u8)> {
    let k = read_varint(r)?;
    Ok((k >> 3, (k & 7) as u8))
}

fn skip<R: Read + Seek>(r: &mut BufReader<R>, wire: u8) -> Result<()> {
    match wire {
        0 => {
            read_varint(r)?;
        }
        1 => r.seek_relative(8)?,
        2 => {
            let n = read_varint(r)?;
            r.seek_relative(n as i64)?;
        }
        5 => r.seek_relative(4)?,
        w => return Err(FaceSwapError::ModelShape(format!("protobuf wire type {w}"))),
    }
    Ok(())
}

/// A TensorProto's dims, type, name and values (raw or float_data).
fn parse_tensor(buf: &[u8]) -> Option<Initializer> {
    let mut r = std::io::Cursor::new(buf);
    let (mut dims, mut ty, mut name) = (vec![], 0u64, String::new());
    let (mut raw, mut floats): (Option<&[u8]>, Vec<f32>) = (None, vec![]);
    while (r.position() as usize) < buf.len() {
        let (field, wire) = read_key(&mut r).ok()?;
        match (field, wire) {
            (1, 0) => dims.push(read_varint(&mut r).ok()? as i64),
            (1, 2) => {
                let n = read_varint(&mut r).ok()?;
                let end = r.position() + n;
                while r.position() < end {
                    dims.push(read_varint(&mut r).ok()? as i64);
                }
            }
            (2, 0) => ty = read_varint(&mut r).ok()?,
            (4, 2) => {
                let n = read_varint(&mut r).ok()? as usize;
                let at = r.position() as usize;
                floats = buf
                    .get(at..at + n)?
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b))
                    .collect();
                r.set_position((at + n) as u64);
            }
            (8, 2) => {
                let n = read_varint(&mut r).ok()? as usize;
                let at = r.position() as usize;
                name = String::from_utf8_lossy(buf.get(at..at + n)?).into_owned();
                r.set_position((at + n) as u64);
            }
            (9, 2) => {
                let n = read_varint(&mut r).ok()? as usize;
                let at = r.position() as usize;
                raw = Some(buf.get(at..at + n)?);
                r.set_position((at + n) as u64);
            }
            (_, 0) => {
                read_varint(&mut r).ok()?;
            }
            (_, 1) => r.set_position(r.position() + 8),
            (_, 2) => {
                let n = read_varint(&mut r).ok()?;
                r.set_position(r.position() + n);
            }
            (_, 5) => r.set_position(r.position() + 4),
            _ => return None,
        }
    }
    let data = match (ty, raw) {
        (FLOAT, Some(raw)) => raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect(),
        (FLOAT16, Some(raw)) => raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| half::f16::from_le_bytes(*b).to_f32())
            .collect(),
        (FLOAT, None) => floats,
        _ => return None,
    };
    let want: i64 = dims.iter().product();
    (data.len() as i64 == want).then_some(Initializer { name, dims, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }

    fn field(num: u64, payload: &[u8], out: &mut Vec<u8>) {
        varint(num << 3 | 2, out);
        varint(payload.len() as u64, out);
        out.extend_from_slice(payload);
    }

    /// A hand-built model with two initializers of the asked shape: the
    /// last one wins, the wrong-shaped one is passed over.
    #[test]
    fn reads_the_last_matching_initializer() {
        let tensor = |name: &str, dims: &[u64], vals: &[f32]| {
            let mut t = vec![];
            for d in dims {
                varint(1 << 3, &mut t);
                varint(*d, &mut t);
            }
            varint(2 << 3, &mut t);
            varint(FLOAT, &mut t);
            field(8, name.as_bytes(), &mut t);
            let raw: Vec<u8> = vals.iter().flat_map(|v| v.to_le_bytes()).collect();
            field(9, &raw, &mut t);
            t
        };
        let mut graph = vec![];
        field(2, b"g", &mut graph);
        field(5, &tensor("a", &[2, 2], &[1., 2., 3., 4.]), &mut graph);
        field(5, &tensor("odd", &[4], &[0.; 4]), &mut graph);
        field(5, &tensor("b", &[2, 2], &[5., 6., 7., 8.]), &mut graph);
        let mut model = vec![];
        varint(1 << 3, &mut model); // ir_version
        varint(8, &mut model);
        field(7, &graph, &mut model);
        let dir = std::env::temp_dir().join(format!("onnx-init-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.onnx");
        std::fs::write(&path, &model).unwrap();
        let t = find_initializer(&path, &[2, 2]).unwrap();
        assert_eq!(t.name, "b");
        assert_eq!(t.data, vec![5., 6., 7., 8.]);
        assert!(find_initializer(&path, &[3, 3]).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
