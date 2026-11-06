// C ABI over the shared C++ video engine core, so Rust can drive the same
// Engine the phone apps use (playback clock, project parsing, timeline math).
// Compiled by build.rs only when the `cpp-engine` feature is on and
// REEL_ENGINE_DIR points at modules/video-engine/cpp of the main repo.

#include <cstdint>
#include <cstring>
#include <optional>
#include <string>

#include "videoengine/Engine.h"
#include "videoengine/FilterPresets.h"
#include "videoengine/Keyframes.h"
#include "videoengine/Timeline.h"

using namespace videoengine;

struct ReelEngine {
  std::string lastError;
  std::optional<Project> snapshot;
  Engine engine;

  ReelEngine()
      : engine(EngineCallbacks{
            nullptr, nullptr, nullptr, nullptr, nullptr, nullptr,
            [this](const EngineError& e) { lastError = e.message; },
        }) {}
};

extern "C" {

typedef struct {
  const char* clip_id;
  const char* source;
  int64_t source_time;
  int64_t offset_into_clip;
  int32_t kind;   // videoengine::ClipKind
  int32_t track_type;  // videoengine::TrackType
  int32_t track_order;
} ReelActiveLayer;

ReelEngine* reel_engine_new(void) {
  auto* e = new ReelEngine();
  e->engine.Initialize();
  return e;
}

void reel_engine_free(ReelEngine* e) {
  if (!e) return;
  e->engine.Dispose();
  delete e;
}

// 0 = loaded; otherwise reel_engine_last_error() says why.
int32_t reel_engine_load_project(ReelEngine* e, const char* json) {
  e->lastError.clear();
  e->engine.LoadProject(json ? json : "");
  if (!e->lastError.empty()) return 1;
  e->snapshot = e->engine.CurrentProjectSnapshot();
  return e->snapshot ? 0 : 1;
}

const char* reel_engine_last_error(ReelEngine* e) { return e->lastError.c_str(); }

void reel_engine_play(ReelEngine* e) { e->engine.Play(); }
void reel_engine_pause(ReelEngine* e) { e->engine.Pause(); }
void reel_engine_seek(ReelEngine* e, int64_t micros) { e->engine.Seek(micros); }
int64_t reel_engine_tick(ReelEngine* e) { return e->engine.TickPlaybackClock(); }
int32_t reel_engine_state(ReelEngine* e) { return static_cast<int32_t>(e->engine.CurrentPlaybackState()); }
int64_t reel_engine_duration(ReelEngine* e) { return e->snapshot ? e->snapshot->duration : 0; }

// Fills `out` (back-to-front) with the clips visible/audible at `micros`,
// source time already resolved through speed / speed curve / reverse /
// freeze. Pointers stay valid until the next reel_engine_load_project.
size_t reel_engine_active_layers(ReelEngine* e, int64_t micros, ReelActiveLayer* out, size_t cap) {
  if (!e->snapshot) return 0;
  auto active = FindActiveClips(*e->snapshot, micros);
  size_t n = 0;
  for (const auto& a : active) {
    if (n >= cap) break;
    ReelActiveLayer& l = out[n++];
    l.clip_id = a.clip->id.c_str();
    l.source = a.clip->source.c_str();
    l.offset_into_clip = a.offsetIntoClip;
    l.source_time = ClipOffsetToSourceTime(*a.clip, a.offsetIntoClip);
    l.kind = static_cast<int32_t>(a.clip->kind);
    l.track_type = static_cast<int32_t>(a.track->type);
    l.track_order = a.track->order;
  }
  return n;
}

// The clip's keyframed `property` ("transform.x", "mask.feather",
// "adjust.contrast", "audio.volume", …) at clip-local `local`, eased as the
// renderers ease it (angles as angles). 1 when the clip has keyframes for it
// (`out` set), 0 when it doesn't (or there is no such clip).
int32_t reel_engine_clip_property(ReelEngine* e, const char* clip_id, const char* property, int64_t local,
                                  double* out) {
  if (!e->snapshot || !clip_id || !property) return 0;
  const std::string id(clip_id), name(property);
  for (const auto& track : e->snapshot->tracks) {
    for (const auto& clip : track.clips) {
      if (clip.id != id) continue;
      const ClipAnimation anim = ParseClipAnimation(clip);
      std::vector<const KeyframeTrackData*> all = {&anim.x, &anim.y, &anim.scale, &anim.rotation, &anim.opacity,
                                                   &anim.volume};
      for (const auto& t : anim.adjust) all.push_back(&t);
      for (const auto& t : anim.mask) all.push_back(&t);
      for (const auto& t : anim.other) all.push_back(&t);
      for (const KeyframeTrackData* t : all) {
        if (t->keyframes.empty() || t->property != name) continue;
        const bool angle = name.size() >= 8 && name.compare(name.size() - 8, 8, "rotation") == 0;
        *out = EvaluateTrack(*t, static_cast<double>(local), 0, angle);
        return 1;
      }
      return 0;
    }
  }
  return 0;
}

// A Classic filter's 4x5 colour matrix (rows R, G, B, A; column 5 the
// offset; straight 0..1 colour) at an intensity (0..100). 0 when it is the
// identity (an unknown name, or intensity 0), else 1.
int32_t reel_filter_color_matrix(const char* name, double intensity, double* out) {
  auto m = GetFilterColorMatrix(name ? name : "", intensity);
  for (size_t i = 0; i < m.size(); ++i) out[i] = m[i];
  return m == kIdentityColorMatrix4x5 ? 0 : 1;
}

}  // extern "C"
