// C ABI over the shared C++ video engine core, so Rust can drive the same
// Engine the phone apps use (playback clock, project parsing, timeline math).
// Compiled by build.rs only when the `cpp-engine` feature is on and
// REEL_ENGINE_DIR points at modules/video-engine/cpp of the main repo.

#include <cstdint>
#include <cstring>
#include <optional>
#include <string>

#include "videoengine/Engine.h"
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

}  // extern "C"
