# Voice — design (planned, not built)

🟦 CLAUDE · 2026-11-04 · Status: **parked for later**. The owner asked for the plan only, no implementation.
Tracked in vault `02 Projects/Papertrail Command Center/TASKS.md` (LATER) and decision D022.

## Intent

The app sits in the tray all day. The owner wants it to *tell* them what matters instead of having to open it:
new problems as they happen, a short spoken briefing on demand, and later a weekly progress read-out from the
planned **Calendar** (separate project, see "Future hooks"). The voice is an output channel on top of data the
app already has; it adds no new data of its own.

## Goals

1. **Spoken alerts:** every desktop alert (`alerts.rs`: new blockers, indexing problems, daily stale digest) can
   also be read aloud.
2. **"Brief me":** a button on the home page (and in the tray menu) speaks a 15–30 second summary built from the
   existing briefing data (`api.briefing()`): what changed since last seen, tasks completed, what's blocked, what's gone quiet.
3. **Settings:** voice on/off (default **off**), voice picker, speed, volume, Test button, quiet hours.
4. **Swappable engine:** one `speak(text)` entry point so a better voice can replace the Windows one later without
   touching callers.

## Non-goals (this pass)

- Listening / speech recognition / conversation. Win+H dictation already works in text boxes. A Whisper
  push-to-talk mode is a possible later phase, not in scope.
- An AI-written briefing. The script is assembled from templates, so it's deterministic, free and offline.
- Reading whole documents (handoffs, logs) aloud.

## Anti-goals (would count as failure even if it works)

- Talking over itself, repeating the same alert, or speaking a flood after a long refresh. At most one queued
  alert summary per refresh, and the same baselines as `alerts.rs` so nothing is announced twice.
- Speaking during quiet hours or when Windows Focus / Do Not Disturb is on (*verify Focus detection is possible*;
  if not, quiet hours alone).
- Any cost or account the owner didn't opt into. Tier 1 is free and offline.
- Making the app heavier on a machine that is already short on RAM. Tier 1 adds no model and no new background process.

## Tiers (build in order; stop whenever it's good enough)

| Tier | Engine | Quality | Cost | Est. effort |
|---|---|---|---|---|
| 1 | Built-in Windows voices | clear, robotic | free, offline | ~½ day |
| 2a | Kokoro (local neural, ~300 MB model, runs as a sidecar process) | near-human | free; RAM/CPU while speaking | ~1 day |
| 2b | ElevenLabs (cloud; API key in Settings) | best; pick/clone a voice | ~$5–22/mo | ~½ day |

2a and 2b are alternatives; the owner picks one when Tier 1 has been lived with.

## How (Tier 1)

**Where speech runs, decided at build time by a 15-minute spike:**
- Option A: in the webview, using `speechSynthesis` (zero dependencies).
- Option B: in Rust, using the Windows `SpeechSynthesizer` through the `windows` crate or the `tts` crate.

**Risk:** when the window is hidden to the tray, WebView2 may throttle or suspend it, so speech that starts from a
Rust event while hidden might never play. The spike tests exactly this: with the window hidden to the tray, emit an
event and check whether it speaks. If it does, use A. Otherwise use B, with speech fired from the worker next to
the existing notification call.

**Pieces:**
1. `speak(text, {priority})`: one entry point with a queue. An alert that arrives while the briefing is playing
   waits for it to finish, and a newer alert replaces any alert still queued. It also checks quiet hours and the
   on/off setting.
2. **Alerts:** `indexer.rs` already has the `(title, body)` pairs where it calls
   `app.notification()…show()`, so it also emits them as `voice-alert`, or calls the Rust speaker with option B.
   Spoken form: "New blocker on OARA: …", with the body cut to about 140 characters.
3. **Brief me:** a template over `api.briefing()`, for example "Since yesterday: 4 commits across 2 projects, 3
   tasks done. Blocked: OARA. Quiet for a week: 12 projects." It drops sections that are empty.
4. **Settings:** keys `voice` (bool, default false), `voice_name`, `voice_rate` (0.8–1.4), `voice_volume`, and
   `voice_quiet` (`{from:"22:00", to:"08:00"}`). Defaults go in `registry.rs` beside `notifications`. The voice
   list comes from the engine itself, so it's whatever is installed.
5. **Tray menu:** add "Brief me" and "Mute voice" items.
6. **Test:** a unit test for the briefing template (empty data says nothing; plurals are right) and for the queue
   rules. A manual check that alerts are spoken with the window hidden to the tray.

## How (Tier 2, when chosen)

- **Kokoro:** bundle it as a Tauri sidecar (an ONNX runtime exe plus the model). `speak` sends text, gets a WAV back
  and plays it. The model downloads on first enable rather than in the installer. **Check:** RAM use while
  speaking; it must not cause the app to crash under memory pressure.
- **ElevenLabs:** the request is made from Rust (the key never reaches the webview) and the key is stored the same
  way other secrets are, never in the vault. Generated audio is cached by text hash so repeated phrases cost
  nothing. If the network fails, it falls back to the Tier 1 voice.

## Future hooks

- **Calendar (separate future project; owner 2026-11-04):** a week view of what was accomplished and projected
  completion of open tasks based on how long they take. When it exists, add a "This week" spoken summary
  ("You closed 9 tasks; OARA's NOW list projects done by Friday") that uses the same `speak` and the same template
  approach. The voice does no calendar work itself.
- **Listening:** Whisper push-to-talk for "what's blocked?" or "open OARA", if wanted later.

## Left to the builder

The spike's choice between A and B; the exact briefing wording; the queue length; the speed range; and where
quiet-hours time zones come from (the local clock).

## Calls Claude made (owner did not decide these; change freely)

- Voice **off by default**, Tier 1 first, template-based briefing rather than AI-written.
- Spoken alerts reuse the existing alert baselines instead of having their own.
- Listening is out of scope.
