# Product tour receipt

[Watch the video](waifu-studio-showcase.mp4) · [Storyboard](storyboard.json) · [Timeline](timeline.json)

76-second narrated tour, 2026-10-18. Opus5.5 wrote the storyboard from inspected feature descriptions; the integrating agent corrected wording, generated narration, recorded the app, edited and checked the output.

- **Footage:**1139 actual native application renders, captured offscreen at15fps. Includes animated Hikari,22-character library, live avatar preview, Scene, Voice, Memory and a seated VRMA clip.
- **Narration:** real Fish hosted synthesis, Drama3, using the currently selected saved reference voice. No voice ID or key is included. Each chapter was synthesized with the production buffered adapter; the visible demo voice-settings page is an isolated fixture, not the account configuration used to record narration.
- **Mouth movement:** prerecorded narration is consumed through the production metered decoder at the recording frame rate and drives the normal expression envelope. This is not a microphone/speaker roundtrip or a perceived synchronization certification.
- **Edit:** chapter titles/subtitles and an explicit offscreen/prototype footer. No fake UI screens or generated avatar animation footage.
- **Desktop claim:** the seated clip is a pose preview inside the studio. No physical taskbar/window interaction is demonstrated.
- **Format:**1280×832 H.264/AAC,75.933seconds, about4MB. All1139 frames and audio decoded successfully. Six representative chapter frames were visually inspected.

The app ran against a separate credential-free fixture directory. No personal conversations, API keys, account balance or saved voice IDs appear in the footage. The UI reports that the fixture is not connected; the narration describes implemented features rather than pretending this is a recorded live chat session.

## Recording path

`examples/synthesize_narration.rs` performs explicit paid synthesis with `--run --storyboard FILE --output-dir EMPTY_DIR`; it preserves the selected voice and refuses silent substitution. `--offscreen-showcase PLAN.json --data-dir ISOLATED_DIR --capture-size 1280x720` records actual app frames without desktop control. It requires an explicit separate directory without a credentials file, an empty frame directory, valid audio and a bounded plan. No output audio device is used.

Plan shape:

```json
{
  "fps": 15,
  "frames_dir": "/absolute/empty-frame-directory",
  "shots": [{
    "frames": 180,
    "screen": "chat",
    "audio": "/absolute/narration.wav",
    "environment": "rose-bedroom",
    "motion": "/absolute/idle-maid.vrma"
  }]
}
```

The fixed simulation clock and per-frame sample count keep the offline recording deterministic in timing. Recording itself ran slower than playback speed; this is not an app frame-rate benchmark. Warm-up frames between chapters are omitted. FFmpeg adds title bands, pads each narration chapter to its shot duration and encodes the movie. Raw frame and narration intermediates are not bundled.
