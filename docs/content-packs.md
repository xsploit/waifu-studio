# Content packs

Content packs add local content descriptions and assets to Waifu Studio through an `extension.json` manifest. Content installation supports validation, enable/disable state, and removal from the installed list. Local executable integrations use a separate explicit installation action, described below. Content activation uses the host's existing importers and UI; a valid manifest alone does not prove that every described feature is active in a particular build.

The included example is `assets/extensions/companion-starter/`, package `org.waifu-studio.companion-starter`, version `1.0.0`. It contains three unchanged, original companion BVHs with explicit MIT asset attribution, an optional built-in Cozy Room reference, and an optional typed preset that enables idle motion. It includes no character model, external assets, generated clips, executable code, or expression mappings. The BVHs animate the head/chest rather than the arms; they have no root translation or locomotion. See the pack's `ATTRIBUTION.md` and `LICENSE` for redistribution terms.

## Install a folder

1. Obtain or unpack a content pack into its own local folder outside the application's managed extension store. Keep the entire folder together, including its license and attribution files.
2. Open **Extensions → Add… → Content pack from folder…** to choose the folder containing `extension.json`, rather than an individual motion or the parent `assets/extensions` folder. For the starter example, select `assets/extensions/companion-starter`. Choose **Add… → Companion starter motions** to install the included example directly.
3. The installer validates the source, copies the whole folder into a managed revision, validates that copy again, and registers the package. A new installation is enabled; installing the same package ID creates a new revision and preserves its existing enabled/disabled state. Consult the host's status/error display if installation fails.
4. Enabled motions join **Scene & avatar** automatically according to their role. On the Extensions page, choose content from the dropdown and press **Use** to preview a motion, import/select a character, choose a background, or apply a preset. Installation and enablement alone do not apply a preset, select a room, replace an existing character, or guarantee automatic playback. A declared `loopable: false` prevents automatic ambient-loop selection. Manual preview holds playback until you resume the automatic animator.

Installation is local and folder-based; no download, provider, generation, shell command, or package-manager step is required. Package folders must contain regular files/directories, not symlinks. Asset references must point to files within the package using portable relative paths, such as `motions/talking-nod.bvh`. Absolute paths, `..`, and external-file references are rejected. The installer copies ancillary files too, so include only intended content and its documentation.

## Current format and support

`src/extension_manifest.rs` defines the strict API-v1 schema. Unknown fields and enum values are rejected. The root has `api_version`, a dotted lowercase package `id`, semantic `version`, `name`, `description`, `provenance`, and optional/defaulted `content`. Provenance records an author, license, and optional source. All local content IDs must be unique across the pack; resolved IDs are `package.id/local-id`.

Supported content descriptions are:

| Kind | Descriptor | Support boundary |
|---|---|---|
| Character | Local `.vrm`/`.glb`, persona, greeting | Existing model import and host selection are still required; installation alone creates no active character. |
| Motion | Local `.bvh`/`.vrma`/`.fbx`, role `idle`, `talking`, or `gesture`; optional emotion/loopability metadata | Existing motion importer and host playback/playlist controls consume these assets. A role or `loopable` label is not motion validation or a safety guarantee. |
| Expression | Existing avatar expression name, host emotion, strength from 0 to 1 | Enabled mappings apply to matching reply emotions when the selected avatar supplies the expression. Missing/reserved expressions fall back to built-in aliases; manual facial channels keep priority. Disabling or updating a mapping revokes its owned contribution. This starter pack supplies none. |
| Background | Local PNG/JPEG/WebP or a supported built-in room | Reuses existing host image/room features. A room reference provides no new renderer implementation. |
| Preset | Typed optional settings: `background` RGB values from 0 to 1, `portrait`, `idle_motion`, `outlines`, `room_view` | Only explicit supported settings; omitted values leave user settings alone. Presets are not arbitrary configuration merges and do not define providers or credentials. |

Supported built-in room names are `cozy_room`, `moonlit_garden`, `neon_rooftop`, `rose_bedroom`, `violet_studio`, and `cyan_lounge`. The starter uses only `cozy_room`. Its preset sets only `idle_motion: true`; applying that preset does not itself select Cozy Room.

The manifest validator checks metadata, paths, and supported asset extensions; it does not decode all media or certify rendered behavior. Existing importers may still reject a file. In particular, this starter pack makes no avatar-specific collision, seamless-loop, transition, or visual-acceptance guarantee.

## Local integrations

The ordinary content-folder installer still rejects `process`, `hooks`, and `capabilities`. Use **Extensions → Add… → Local integration from folder…** for a package containing one bundled executable program (including an executable script with a shebang). Program and working-directory paths stay inside the copied package; arguments are literal argument values, with bounded sizes. Alternatively, use one to four unique event hooks for reply completion, speech start/stop or emotion changes; a pack cannot combine hooks with a persistent program.

A local integration runs code with your operating-system account: **it is not sandboxed**. Host permissions control access to Waifu Studio actions and event payloads, not all filesystem or network access. Install only code you trust. Provider keys are not forwarded through the child environment.

New integrations install disabled with no host permissions. Open **Permissions** on the installed card to allow the requested features, then enable the installed card. Enabled integrations restore on app startup. Status stays on that card; **Advanced → Restart** (or **Reset** for hooks) contains process controls. Disabling, removing or replacing a revision stops its process and revokes its owned temporary effects. A failed process is not automatically retried every frame; use Restart or disable and enable to retry.

Updates retain the installation identity and permissions still requested by the new revision; newly requested permissions remain off. Persistent writable data belongs to the installation, separate from immutable revisions. Manifest-defined plain settings support toggles, bounded integers, text, choices, registered-motion selectors and avatar-expression selectors. Saved values survive restart and compatible updates. Saving changes restarts an enabled integration with a fresh session; unsaved drafts do not affect it. Removed or incompatible settings return to the new defaults during update. Declare credentials separately and use the private **Credentials** controls; never put passwords or API keys in plain settings.

The included **Lifecycle observer** example (`assets/extensions/lifecycle-observer`) requires Python 3 and requests lifecycle metadata only. Its data folder contains a single replaced `latest-event.json` receipt. It does not request private reply text or avatar control. Process supervision currently supports Linux; other platforms report unsupported execution. Attributed external messages are persisted as third-party context for the next normal conversation request. The host user can separately enable **Reply to incoming messages** for the loaded character in Extensions. It defaults off and preserves drafts, attachments and user-memory intake boundaries; accepted delivery remains successful even if the reply cannot start. A global ten-second running-app cooldown and ordinary busy checks apply, without an extra reply backlog or automatic retry. A native Twitch receiver is implemented below; live account acceptance remains pending. Discord remains unfinished.

## Disable, update, and remove

Disabling preserves the installed package and its files while excluding it from the active pack catalog. It is not a command to erase previously imported content or undo settings the user already applied. Enabling revalidates the installed copy. Disabled pack motions leave the active catalog, and their pending/playing poses are revoked without releasing an explicit manual hold. Pack expression contributions are revoked as well.

An update creates a fresh managed revision rather than overwriting the files an existing character, animation, or saved setting may still reference. The local extension store tracks installations in `installed.json` and keeps copied revisions under `packages/<installation-id>/<revision>/`. The store's base directory is selected by the host; do not hardcode a platform-specific location into a pack.

Removal unregisters the pack from the installed list. **Copied package files and old revisions are retained**, including assets that user settings or imports may still reference. The original source folder is also unchanged. Removal does not delete customized characters or restore previously applied presets. Disk cleanup and migrating retained references are separate tasks; removal is not a disk-reclamation guarantee.

The store has one exclusive writer. A second app using the same extension store receives a readable busy error. Interrupted installer staging files are cleaned when the lock is acquired; retained installed revisions are preserved. If an earlier background copy is still finishing, refresh after it releases the lock.


## Integration settings schema

Only local integrations can declare `settings`. Each entry has a unique `id`, `label`, optional `description` and a typed default. Examples:

```json
"settings": [
  {"id":"active","label":"Active","type":"toggle","default":true},
  {"id":"cooldown","label":"Cooldown seconds","type":"integer","min":1,"max":60,"default":5},
  {"id":"label","label":"Label","type":"text","max_length":128,"default":"Observer"},
  {"id":"group","label":"Event group","type":"choice","options":["all","speech"],"default":"all"},
  {"id":"gesture","label":"Gesture","type":"motion","default":""},
  {"id":"face","label":"Expression","type":"expression","default":""},
  {"id":"character","label":"Character","type":"character","default":""}
]
```

Limits: 32 fields, 64-byte field IDs, text at most 1024 UTF-8 bytes, 32 unique choices, and 16 KiB for the complete serialized values. Unknown fields, wrong types, out-of-range values and invalid defaults are rejected. The host startup envelope carries the effective values as `message.settings`, independently of host capability grants. These are plain values persisted in the local extension index, not secret storage.


Character settings use the library character dropdown. **Loaded character** stores an empty string; choosing a specific character stores its canonical UUID. Missing characters remain saved and are labelled unavailable. This is a filter for the handler, not permission to change the loaded avatar or address another target.

## Configurable reaction hook

Choose **Extensions → Add… → Examples → Configurable reaction hook** for one compact mapping: **Event**, **Character**, **Gesture** and **Expression**. The bundled Python handler supports completed replies, speech starting/stopping and emotion changes. Character filters compare the source event with the loaded target; picking another character never switches avatars. The default is the loaded character, completed replies and a gentle nod, with no expression.

Save settings before using **Test reaction**. The preview uses the saved reaction and character filter without needing a new reply or provider call. Both the normal hook and preview use the public action protocol, current grants and host ownership rules. A successful action acknowledgement is not motion completion. Use None to omit a gesture or expression. The example runs local code and installs disabled without grants, like other executable examples.

## Reply gesture example

Choose **Extensions → Add… → Examples → Reply gesture**. This owned Python 3 integration includes an original gentle head/chest nod. It installs disabled without grants. Allow **Observe reply and speech activity** and **Play motions**, then enable it. Settings select completed replies or speech starts, a cooldown and a registered Gesture. None disables gesture requests. It reacts only when the event's source character matches the loaded target. It requests one registered gesture and waits for its correlated `motion.finished` result; busy/rejected requests are recorded without automatic retries.

The process writes one replaced `latest.json` receipt in its installation data directory. It requests no private reply text or provider key. It runs local code without an OS sandbox. It demonstrates event-driven behavior in a persistent integration; the separate Reply gesture hook example demonstrates manifest-defined one-shot processes. This motion is upper-body only and establishes no universal collision guarantee.


## One-shot event hooks

Choose **Extensions → Add… → Examples → Reply gesture hook**. Grant observation and motion playback, then choose the **Enabled** switch. Its Gesture selector chooses a registered motion from enabled packs; None exits without requesting a gesture. An enabled hook waits without a running child until its configured event arrives. The included Python handler requests its registered nod, waits for the actual motion terminal receipt, and exits. It needs Python 3 and runs local code without an OS sandbox.

Integration manifests may contain either one persistent `process` or one to four `hooks`, with unique `reply.finished`, `speech.started`, `speech.stopped`, or `emotion.changed` triggers. Each handler uses the same contained executable, arguments, and working-directory validation as a persistent program.

Hooks share four process slots with persistent integrations and terminating children. The queue holds at most 32 invocations and 1 MiB; queued work expires after five seconds. Each child has a ten-second lifetime. One invocation starts per frame, and one child per installation runs at a time. Delivery is bounded, not guaranteed: overflow, expiry and launch failures are reported without automatic replay.

Events capture their source and target before launch. Observation permission is required; private reply text additionally requires read permission. Revoking permissions permanently removes those permissions and private text from queued captures. Target changes, disablement, removal and configuration replacement discard pending work. **Reset** discards queued work, stops the current child and waits for new events. A source-journal gap stops the subscription until explicit reset or re-enablement. Overflow/expiry counts and the last loss reason remain visible across later launches and Reset; removing/reconfiguring the registration clears them.


## Emotion observations

Persistent integrations with observation permission can receive `emotion.changed`. The event identifies the original character, independent focused-chat or scene-chat affect store, previous/current emotion labels, and original reply ID when known. It carries no reply text or current avatar target. Accepted label changes and explicit neutral resets emit once; same-label mood updates and stale/failed appraisals do not emit. An applied affect label does not guarantee a visible expression, because manual controls and available avatar expressions still take precedence. One-shot manifest hooks also support `emotion.changed`, with the same source metadata and observation permission. Emotion events supply no current action target.


## Attributed messages and offline Twitch replay

A local integration with **Deliver external messages** permission can deliver bounded text to the current loaded character. It appears with an external integration label and retains its host-assigned identity and origin after reopening history. Delivery does not start a reply, speech, memory extraction or decision-model triage. A later ordinary conversation request includes it as quoted third-party data rather than a claim from the local user.

Delivery rejects stale targets, missing permission, active focused/scene generation and history mismatches. Success confirms the retained candidate is visible in storage; directory-sync errors reconciled by readback do not establish crash durability. An unconfirmed external save blocks further focused delivery/sends until a successful conversation reload. A cancelled scene worker remains a history writer until it actually exits.

Choose **Add… → Examples → Twitch replay (offline)** for an owned Python 3 fixture. Installation starts disabled without grants. Grant external delivery, observation and motion playback, enable it with a loaded avatar, and it replays one synthetic subscription, cheer or chat event. It waits for message acceptance, requests the bundled gentle nod, then waits for actual playback completion. **Restart** explicitly replays the selected example. This sample needs no account, network or provider key and is not a live Twitch connection. Settings choose the synthetic event; no real account data is bundled.


## Twitch receiver and private credentials

See [Twitch setup and acceptance](twitch-setup.md) for account binding, exact scope selection, reaction testing and remaining live checks.

Choose **Add… → Twitch**. The bundled Python 3 launcher starts a separate native Waifu Studio worker. Install, grant observation/external delivery/motion playback, and configure the folded **Credentials** section before enabling. Supply your registered Twitch application ID and a user access token for the broadcaster account privately in the app. The event dropdown selects chat and alerts, alerts only, or chat only. **Alert gesture** selects a registered motion from enabled packs; the bundled thanks nod is the default and **None** disables motion requests while retaining external messages. Required scopes depend on selection: `user:read:chat`, `channel:read:subscriptions`, and `bits:read` respectively.

Credentials are stored separately in the installation data directory with owner-only Unix permissions. The UI displays Stored/Not set and empty masked replacement fields; blank fields preserve existing values. Saving a real change restarts the integration. Provider keys are not forwarded. Installed programs run with the user's privileges, so only install code you trust.

The receiver validates token identity/scopes at startup and hourly, subscribes through Twitch EventSub, and observes chat, subscriptions and cheers. It receives events only: it does not post Twitch chat. The separate host-owned incoming-message opt-in can start ordinary replies after successful context delivery. Accepted events appear as attributed external context; eligible celebrations request the original gentle nod and wait for actual completion. Stale-target and uncertain outcomes are reported without action replay. The receiver now keeps a16-notification FIFO backlog for15 seconds and retries only explicitly refused Busy message deliveries, at most three attempts one second apart. Accepted deliveries and expression/motion failures are never retried. Target changes discard pending work; overflow/expiry is reported. Graceful reconnect preserves the subscription; a lost connection reports the possible event gap.

Explicit device sign-in is implemented in Credentials: enter a registered public-client application ID, choose Connect with Twitch, then Open browser and approve in Twitch. This saves access/refresh credentials without enabling the installation or adding grants. Configuration changes/cancellation discard pending results. For device-authorized credentials, host-owned automatic refresh uses private saved identity/expiry metadata and the same cancellation/configuration guards. Rotation is saved atomically, then the enabled receiver restarts; event gaps are possible. Manual credential changes invalidate OAuth metadata. Unknown refresh outcomes require explicit sign-in rather than automatic retries. Live Twitch account/network acceptance remains pending; offline fixtures and source implementation are separate from successful real-account operation.

Duplicate protection retains at most 256 unique EventSub envelope IDs for 330 seconds, including control traffic. Capacity exhaustion stops the worker with a diagnostic rather than evicting live IDs or reconnecting to repeat actions. This conservative limit is unsuitable for busy channels without further capacity work; Restart is explicit and resets the process cache.


## Emotion hook example

Choose **Add… → Examples → Emotion hook**, grant observation, and enable hooks. This Python 3 handler starts only for an accepted emotion label change, writes one replaced `latest.json` metadata receipt in its private installation data directory, and exits. It requests no private reply text, provider key, expression or motion control. The receipt preserves the source character, focused/scene affect owner, previous/current labels and optional genuine reply correlation. It does not infer an action target from that source.

Emotion hooks share the existing process slots, queue bounds, expiry, cancellation and permission rules. Same-label mood updates are not new source events. Replaying a consumed source sequence cannot start another invocation; revoking observation discards queued captures permanently even if permission is subsequently restored.


## Registered motion selectors

A setting with `"type":"motion"` uses a compact searchable dropdown populated from enabled, resolved content packs. The stored text is a registered motion ID, with an empty string meaning **None**. The maximum is 193 UTF-8 bytes with no whitespace/control characters, matching motion actions. Motion IDs are references, never file paths or permission grants.

Unavailable selections remain saved and visibly unavailable when a contributing pack is disabled or removed. Re-enable the pack or choose another motion; unrelated saves do not silently substitute an animation. The host still validates the actual action against the current target, active catalog and motion permission. Selecting a motion does not enable its pack, grant control, or guarantee collision-free movement.


## Avatar expression selectors

A setting with `"type":"expression"` stores an exact avatar expression name, or empty for **None**. The searchable dropdown uses the loaded avatar's available expressions and excludes blink, mouth and gaze channels owned by the face systems. Names retain casing and internal spaces, fit128 UTF-8 bytes and contain no controls; whitespace-only or reserved names are rejected. Unavailable choices remain saved across avatar changes and unrelated settings updates.

Twitch's optional **Alert expression** requests a temporary75% expression for two seconds, then requests the chosen gesture after the expression action is accepted. It does not wait two seconds before requesting the gesture. Expression control requires its own grant; existing installations do not gain this permission automatically. Without that grant, the existing gesture path still works. None is the expression default. Messages, expressions and motions retain separate host admission and correlation; choosing an expression does not authorize or guarantee visible playback.


## Testing a saved reaction

Integrations can declare an optional `test_process` with the same contained program/argument/working-directory rules as a regular process. It supplements a required persistent process or event hooks; content-only packages cannot include executable tests.

For Twitch, **Test reaction** runs the bundled offline handler using saved settings, existing grants and the loaded character. Unsaved drafts are not used. It does not load Twitch credentials, connect to Twitch or deliver an external chat message. Enable the installation and grant the selected expression/motion permissions first. None/no granted reaction produces no avatar action. Existing network service failure does not by itself prevent this separate local test.

The test shares the four-child process budget, lasts at most ten seconds and cannot overlap another test for the same installation, including children still stopping. It pins the current avatar incarnation, uses ordinary host action checks, waits for actual motion completion and preserves temporary expressions long enough for their preview. Avatar changes, disablement, removal, reconfiguration, permission revocation and reset cancel it. No automatic retry or replay occurs. Long motions can exceed the test lifetime; that is a failed preview, not proof the animation cannot play normally.

The integration card keeps the most recent test process result. A process success alone does not prove native visual playback or a live Twitch event. The owned Twitch handler separately requires an actual correlated motion terminal and records fixed metadata-only states in `latest-test.json`.


## Write an integration

See [Authoring local process and hook integrations](extension-authoring.md) for exact current JSONL envelopes, language-neutral process requirements, hooks, permissions and terminal correlation. The API remains provisional.


## Read-only manifest check

Authors can run `waifu-studio --check-extension /path/to/package` before installation. Use this option alone. The command uses the same manifest and declared-path validators and exits without initializing the app, reading startup credentials, installing or running code. A nonzero exit code reports validation failure. It does not replace full-folder installation checks, asset decoding, visual review or runtime acceptance. See [authoring](extension-authoring.md#check-a-package-without-installing-it).
