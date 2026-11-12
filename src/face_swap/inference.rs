//! ONNX Runtime for the face models: one environment per process, each
//! model's session loaded once and reused, on the best execution provider
//! this machine has (CoreML on macOS, DirectML on Windows, CUDA when built
//! with `--features cuda`), else the CPU. A provider that can't run some
//! op hands that op to the CPU (ONNX Runtime partitions the graph); a
//! provider that fails to start at all falls back to a CPU-only session.
//!
//! Tensors go in and out as plain `f32` (NCHW); fp16 models get their
//! inputs converted. Every input is checked against the model's declared
//! shape before a run, so a wrong file fails with a message, not a crash.

use std::path::Path;
use std::sync::OnceLock;

use ort::environment::Environment;
use ort::ep;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::{Tensor, TensorElementType, ValueType};

use super::error::{FaceSwapError, Result};

/// Where a session runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Cpu,
    CoreMl,
    DirectMl,
    Cuda,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Provider::Cpu => "CPU",
            Provider::CoreMl => "Core ML",
            Provider::DirectMl => "DirectML",
            Provider::Cuda => "CUDA",
        }
    }
}

fn environment() -> Result<&'static Environment> {
    static ENV: OnceLock<Result<Environment>> = OnceLock::new();
    ENV.get_or_init(|| {
        runtime()?
            .with_name("reel-face-swap")
            .with_telemetry(false)
            .build()
            .map_err(|e| FaceSwapError::Inference(e.to_string()))
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// ONNX Runtime: linked in on Apple silicon and Windows, loaded on Linux
/// and Intel Macs (`runtime`).
fn runtime() -> Result<ort::environment::EnvironmentBuilder> {
    #[cfg(ort_dynamic)]
    return super::runtime::load().map_err(FaceSwapError::RuntimeMissing);
    #[cfg(not(ort_dynamic))]
    Ok(ort::init())
}

/// One declared input / output: name, element type, shape (-1 = dynamic).
#[derive(Clone, Debug)]
pub struct Port {
    pub name: String,
    pub ty: TensorElementType,
    pub shape: Vec<i64>,
}

fn ports(outlets: &[ort::value::Outlet]) -> Vec<Port> {
    outlets
        .iter()
        .map(|o| match o.dtype() {
            ValueType::Tensor { ty, shape, .. } => Port {
                name: o.name().to_string(),
                ty: *ty,
                shape: shape.iter().copied().collect(),
            },
            _ => Port {
                name: o.name().to_string(),
                ty: TensorElementType::Float32,
                shape: vec![],
            },
        })
        .collect()
}

/// A loaded model.
pub struct Model {
    session: Session,
    pub provider: Provider,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
}

/// An output tensor's values.
pub struct Output {
    pub data: Vec<f32>,
}

impl Model {
    /// Loads `path`, on the GPU provider when `gpu` and one is there.
    /// `cache_dir` keeps compiled CoreML programs between launches.
    pub fn load(path: &Path, gpu: bool, cache_dir: Option<&Path>) -> Result<Model> {
        let env = environment()?;
        if gpu {
            for provider in gpu_providers() {
                match Self::load_on(env, path, provider, cache_dir) {
                    Ok(m) => return Ok(m),
                    Err(e) => eprintln!(
                        "face swap: {} on {} failed, trying the next: {e}",
                        path.display(),
                        provider.label()
                    ),
                }
            }
        }
        Self::load_on(env, path, Provider::Cpu, cache_dir)
    }

    fn load_on(
        env: &Environment,
        path: &Path,
        provider: Provider,
        cache_dir: Option<&Path>,
    ) -> Result<Model> {
        let err = |e: ort::Error<ort::session::builder::SessionBuilder>| {
            FaceSwapError::Inference(e.to_string())
        };
        let mut builder = Session::builder(env)?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(err)?;
        if let Some(dispatch) = dispatch_for(provider, cache_dir) {
            builder = builder
                .with_execution_providers([dispatch.error_on_failure()])
                .map_err(err)?;
        }
        let session = builder.commit_from_file(path)?;
        Ok(Model {
            inputs: ports(session.inputs()),
            outputs: ports(session.outputs()),
            session,
            provider,
        })
    }

    /// Runs the model on `inputs` (one per declared input, in order:
    /// shape and values). Returns every output as f32.
    pub fn run(&mut self, inputs: Vec<(Vec<i64>, Vec<f32>)>) -> Result<Vec<Output>> {
        if inputs.len() != self.inputs.len() {
            return Err(FaceSwapError::ModelShape(format!(
                "{} inputs given, the model takes {}",
                inputs.len(),
                self.inputs.len()
            )));
        }
        let mut values: Vec<(String, ort::value::DynValue)> = Vec::with_capacity(inputs.len());
        for ((shape, data), port) in inputs.into_iter().zip(&self.inputs) {
            check_shape(port, &shape)?;
            let value = match port.ty {
                TensorElementType::Float32 => Tensor::from_array((shape, data))?.into_dyn(),
                TensorElementType::Float16 => {
                    let half: Vec<half::f16> =
                        data.iter().map(|v| half::f16::from_f32(*v)).collect();
                    Tensor::from_array((shape, half))?.into_dyn()
                }
                other => {
                    return Err(FaceSwapError::ModelShape(format!(
                        "input {} is {other:?}",
                        port.name
                    )));
                }
            };
            values.push((port.name.clone(), value));
        }
        let names: Vec<String> = self.outputs.iter().map(|p| p.name.clone()).collect();
        let outputs = self.session.run(values)?;
        let mut out = Vec::with_capacity(names.len());
        for name in &names {
            let v = &outputs[name.as_str()];
            let data = match v.try_extract_tensor::<f32>() {
                Ok((_, d)) => d.to_vec(),
                Err(_) => {
                    let (_, d) = v.try_extract_tensor::<half::f16>()?;
                    d.iter().map(|h| h.to_f32()).collect()
                }
            };
            out.push(Output { data });
        }
        Ok(out)
    }

    /// An input's declared spatial size (H, W), when fixed.
    pub fn input_hw(&self, index: usize) -> Option<(i64, i64)> {
        let s = &self.inputs.get(index)?.shape;
        (s.len() == 4 && s[2] > 0 && s[3] > 0).then(|| (s[2], s[3]))
    }
}

/// The declared shape must match where it's fixed.
fn check_shape(port: &Port, shape: &[i64]) -> Result<()> {
    let fits = port.shape.is_empty()
        || (port.shape.len() == shape.len()
            && port
                .shape
                .iter()
                .zip(shape)
                .all(|(want, got)| *want < 0 || want == got));
    if fits {
        Ok(())
    } else {
        Err(FaceSwapError::ModelShape(format!(
            "input {} wants {:?}, got {shape:?}",
            port.name, port.shape
        )))
    }
}

/// The GPU providers this build has, best first.
fn gpu_providers() -> Vec<Provider> {
    let mut list = vec![];
    if cfg!(feature = "cuda") {
        list.push(Provider::Cuda);
    }
    if cfg!(target_os = "macos") {
        list.push(Provider::CoreMl);
    }
    if cfg!(windows) {
        list.push(Provider::DirectMl);
    }
    list
}

#[allow(unused_variables)]
fn dispatch_for(
    provider: Provider,
    cache_dir: Option<&Path>,
) -> Option<ep::ExecutionProviderDispatch> {
    match provider {
        Provider::Cpu => None,
        #[cfg(target_os = "macos")]
        Provider::CoreMl => {
            let mut coreml = ep::CoreML::default()
                .with_model_format(ep::coreml::ModelFormat::MLProgram)
                .with_compute_units(ep::coreml::ComputeUnits::All);
            if let Some(dir) = cache_dir {
                let _ = std::fs::create_dir_all(dir);
                coreml = coreml.with_model_cache_dir(dir.display());
            }
            Some(coreml.build())
        }
        #[cfg(windows)]
        Provider::DirectMl => Some(ep::DirectML::default().build()),
        #[cfg(feature = "cuda")]
        Provider::Cuda => Some(ep::CUDA::default().build()),
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_check_fixed_dims_only() {
        let port = Port {
            name: "x".into(),
            ty: TensorElementType::Float32,
            shape: vec![1, 3, -1, -1],
        };
        assert!(check_shape(&port, &[1, 3, 640, 480]).is_ok());
        assert!(check_shape(&port, &[1, 4, 640, 480]).is_err());
        assert!(check_shape(&port, &[3, 640, 480]).is_err());
    }

    /// A one-node ONNX model (`y = Relu(x)`, x: float[3]), written out as
    /// its protobuf by hand.
    fn relu_model() -> Vec<u8> {
        fn varint(mut v: u64, out: &mut Vec<u8>) {
            while v >= 0x80 {
                out.push(v as u8 | 0x80);
                v >>= 7;
            }
            out.push(v as u8);
        }
        fn int(field: u64, v: u64) -> Vec<u8> {
            let mut out = vec![];
            varint(field << 3, &mut out);
            varint(v, &mut out);
            out
        }
        fn bytes(field: u64, body: &[u8]) -> Vec<u8> {
            let mut out = vec![];
            varint(field << 3 | 2, &mut out);
            varint(body.len() as u64, &mut out);
            out.extend_from_slice(body);
            out
        }
        // ValueInfoProto { name, type: { tensor_type: { elem_type: FLOAT, shape: [3] } } }
        let value = |name: &str| {
            let dim = bytes(1, &int(1, 3));
            let tensor = [int(1, 1), bytes(2, &dim)].concat();
            [bytes(1, name.as_bytes()), bytes(2, &bytes(1, &tensor))].concat()
        };
        let node = [bytes(1, b"x"), bytes(2, b"y"), bytes(4, b"Relu")].concat();
        let graph = [
            bytes(1, &node),
            bytes(2, b"g"),
            bytes(11, &value("x")),
            bytes(12, &value("y")),
        ]
        .concat();
        let opset = int(2, 13);
        [int(1, 8), bytes(7, &graph), bytes(8, &opset)].concat()
    }

    /// ONNX Runtime starts and runs a model on the CPU (on Linux: its
    /// library is found and loads). `cargo test runtime_runs_a_model --
    /// --ignored`, with the library next to the test binary or in
    /// `ORT_DYLIB_PATH`.
    #[test]
    #[ignore]
    fn runtime_runs_a_model() {
        let path = std::env::temp_dir().join(format!("relu-{}.onnx", std::process::id()));
        std::fs::write(&path, relu_model()).unwrap();
        let loaded = Model::load(&path, false, None);
        let _ = std::fs::remove_file(&path);
        let mut model = loaded.unwrap();
        assert_eq!(model.provider, Provider::Cpu);
        assert_eq!(model.inputs[0].shape, [3]);
        let out = model.run(vec![(vec![3], vec![-1.0, 0.5, 2.0])]).unwrap();
        assert_eq!(out[0].data, [0.0, 0.5, 2.0]);
    }
}
