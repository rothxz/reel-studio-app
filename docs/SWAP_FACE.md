# Implement Local AI Face Swap — Rust + GPUI

## Objective

Implement a production-oriented, fully local AI Face Swap feature in the desktop video editor built with Rust and GPUI.

The feature must replace a selected person's face in a video clip using a source face image. All AI inference must run locally without cloud APIs.

Prioritize:

- Realistic face swapping
- Temporal consistency across video frames
- GPU acceleration
- Efficient memory usage
- Non-blocking UI
- Modular and maintainable Rust architecture
- Commercially compatible model licensing

## 1. Existing Project Requirements

Before implementing:

1. Inspect the existing Rust workspace and application architecture.
2. Identify the current video decoding, encoding, rendering, timeline, effect, and GPU systems.
3. Identify existing AI inference abstractions and model-loading mechanisms.
4. Reuse existing infrastructure instead of creating duplicate systems.
5. Integrate with the current GPUI interface and timeline architecture.
6. Do not rewrite unrelated components.

Do not assume the project uses a specific video engine or rendering backend. Discover it from the repository.

## 2. AI Model Strategy

### Primary face swap candidate

Evaluate GHOST 256 ONNX.

Repository:
https://github.com/ai-forever/ghost

GHOST 256 is a candidate for one-shot face swapping.

Before integration:

- Verify the exact pretrained weights and license.
- Verify all face embedding and conversion model dependencies.
- Verify commercial redistribution permissions.
- Do not bundle models with unclear or non-commercial licensing.
- Keep model files external to the application binary.

### Face detection

Evaluate YuNet ONNX.

Repository:
https://github.com/opencv/opencv_zoo/tree/main/models/face_detection_yunet

Use a commercially compatible face detector.

### Face alignment

Implement five-point facial landmark alignment:

- Left eye
- Right eye
- Nose
- Left mouth corner
- Right mouth corner

Use similarity transforms to align the face crop to the model's required input geometry.

### Face restoration

Optional restoration model:

- GFPGAN

Repository:
https://github.com/TencentARC/GFPGAN

Restoration must be configurable and disabled by default in performance-oriented preview mode.

Do not assume the detector, embedding model, swapper, and restoration model share the same license.

## 3. Rust Architecture

Create modular components following the existing repository conventions.

Suggested module organization:

```text
src/
  ai/
    face_swap/
      mod.rs
      config.rs
      error.rs
      model_registry.rs
      model_loader.rs
      inference.rs
      face_detector.rs
      face_aligner.rs
      face_tracker.rs
      face_embedder.rs
      face_swapper.rs
      face_restorer.rs
      face_mask.rs
      face_blender.rs
      frame_processor.rs
      video_processor.rs
      cache.rs
      job.rs
      types.rs
```

Adapt the structure to the existing workspace rather than blindly creating every file.

### Core types

Implement appropriate Rust types:

- `FaceSwapConfig`
- `FaceSwapJob`
- `FaceSwapSource`
- `DetectedFace`
- `FaceLandmarks`
- `FaceTrack`
- `FaceSwapResult`
- `FaceSwapProgress`
- `FaceSwapError`

Avoid passing raw unstructured data across the pipeline.

## 4. Inference Runtime

Use ONNX Runtime through the Rust `ort` crate if compatible with the existing project.

Requirements:

- Load models once and reuse sessions.
- Support CPU inference.
- Evaluate GPU execution providers for each target platform.
- Use CoreML on macOS where supported.
- Evaluate CUDA on NVIDIA GPUs.
- Evaluate DirectML on Windows where supported.
- Avoid forcing a GPU provider if it is unavailable.
- Provide graceful CPU fallback.
- Validate model input and output tensor shapes.
- Handle inference errors without crashing the editor.

Do not assume every execution provider supports every ONNX operation.

## 5. Face Swap Pipeline

Implement this processing pipeline:

1. Decode video frame.
2. Detect target faces.
3. Identify the selected face.
4. Track the face across frames.
5. Extract facial landmarks.
6. Align the target face.
7. Prepare the source face identity representation.
8. Run the face swap model.
9. Apply optional restoration.
10. Generate a face blending mask.
11. Perform color correction.
12. Blend the swapped face into the original frame.
13. Return the processed frame.
14. Continue to the next frame.

Preserve the original frame outside the face region.

## 6. Temporal Consistency

Video face swapping must minimize flickering.

Implement:

- Persistent face tracking IDs.
- Source face embedding cache.
- Target face tracking.
- Landmark smoothing.
- Stable face masks.
- Color consistency between adjacent frames.
- Detection refresh intervals.
- Tracking recovery after temporary occlusion.

Do not independently select the largest face on every frame.

The selected face must remain consistent throughout the clip.

If the target face disappears:

- Stop applying the swap to that track.
- Avoid applying the swap to another person's face.
- Resume when the original track is confidently recovered.

Use configurable confidence thresholds.

## 7. Performance Requirements

The feature must not block the GPUI main thread.

Use:

- Background worker threads.
- Bounded frame queues.
- Cancellation tokens.
- Frame batching only when supported and beneficial.
- Memory-aware frame buffers.
- GPU/CPU resource reuse.
- Cache management.

Preview and export must use separate quality configurations.

### Preview mode

- Lower-resolution inference where appropriate.
- Fast processing.
- Optional face restoration disabled.
- Prioritize interactive timeline playback.

### Export mode

- Higher quality processing.
- Full resolution compositing.
- Optional restoration.
- Stable temporal tracking.
- Correct synchronization with video timestamps.

Do not decode an entire video into RAM.

## 8. Timeline Integration

Integrate Face Swap as a non-destructive timeline effect.

The effect should contain:

- Source face image reference.
- Target face track ID.
- Processing configuration.
- Effect enabled/disabled state.
- Preview cache references.
- Model version metadata.

The original video clip must remain unchanged.

The effect should be editable, removable, and reorderable according to the editor's existing effect architecture.

## 9. GPUI User Interface

Implement a Face Swap effect panel.

UI functionality:

### Source Face

- Import source face image.
- Display source face preview.
- Replace or remove source image.

### Target Face

- Detect faces in selected video.
- Display detected faces as selectable thumbnails.
- Select a specific target person.

### Settings

- Face swap strength, if supported by the model.
- Blend strength.
- Face restoration toggle.
- Preview quality.
- Processing quality.

Do not expose parameters that the model cannot actually control.

### Processing

- Progress indicator.
- Cancel button.
- Processing status.
- Error messages.
- Preview result.

All UI state updates must use the existing GPUI state management patterns.

## 10. Caching

Implement caching for:

- Source face embedding.
- Face detection results.
- Facial landmarks.
- Face tracking data.
- Processed preview frames.

Cache keys must include relevant:

- Clip identifier.
- Source face image hash.
- Target face track ID.
- Effect settings.
- Model version.
- Frame timestamp.

Invalidate the cache when the source image, target selection, model, or relevant effect parameters change.

## 11. Model Management

Implement a local model registry.

Requirements:

- Model metadata.
- Model version.
- Model file path.
- File size.
- SHA-256 verification.
- License information.
- Download progress if models are downloaded.
- Model availability status.
- Missing-model error handling.

Do not automatically download models without user consent.

Never download models from unverified URLs.

## 12. Safety and Privacy

- All face swap inference must remain local.
- Do not upload source images or video frames.
- Require user confirmation when applying a face swap to an imported person's face.
- Provide clear indication that the video has been manipulated.
- Do not implement identity impersonation or bypass identity verification systems.

## 13. Testing

Implement tests for:

### Unit tests

- Face detection output validation.
- Landmark alignment.
- Face mask generation.
- Blending.
- Cache invalidation.
- Configuration validation.

### Integration tests

- Source image to target image swap.
- Single-person video processing.
- Multiple-person video with selected target track.
- Face disappearance and reappearance.
- Cancellation.
- CPU inference.
- GPU inference where available.

### Quality tests

- Face identity similarity.
- Facial expression preservation.
- Temporal flicker.
- Blending artifacts.
- Occlusion handling.

Use legally appropriate test images and videos.

## 14. Implementation Workflow

Proceed in phases.

### Phase 1 — Repository Analysis

Inspect existing architecture and document integration points.

### Phase 2 — Model Validation

Validate candidate ONNX models and their licensing.

### Phase 3 — Image Face Swap

Implement a minimal working source-image-to-target-image pipeline.

### Phase 4 — Video Processing

Integrate the pipeline into video frame processing.

### Phase 5 — Temporal Tracking

Implement stable face tracking across frames.

### Phase 6 — GPUI Integration

Implement the Face Swap panel and timeline effect.

### Phase 7 — Performance Optimization

Optimize inference, caching, memory, and GPU utilization.

### Phase 8 — Testing

Validate correctness, performance, and temporal consistency.

## Final Instructions

- Inspect the repository before writing code.
- Use the existing architecture and conventions.
- Implement real functionality rather than mock UI.
- Do not use cloud AI services.
- Do not use Python as a runtime dependency in the shipped application.
- Do not bundle non-commercial models in a commercial application.
- Do not fabricate model capabilities.
- Keep all inference and processing local.
- Explain major architectural decisions.
- Report files created or modified.
- Report build and test results honestly.
- If a model cannot be used legally or technically, explain the blocker and propose a compatible alternative.

Start with Phase 1: repository analysis.
