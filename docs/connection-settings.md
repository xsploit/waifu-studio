# Connection settings

Everyday controls are on the main pages. **Connections** contains the chat
provider/model, creativity and shared decision-model selector. **Voice &
microphone** contains the speech service/model/voice/speed and transcription
service/model. Advanced holds exact IDs, endpoints, key environment-variable
names and transport tuning. Session overrides for chat, decisions, voice and the
microphone are together in **Connections → Keys → Manage…**.

The **Keys** card reports credentials for each feature independently, including
active session keys. **Loaded** means a key is available, not that a provider has
accepted it. Local servers may need no key. **Manage…** opens Advanced directly, with the four masked session-key fields together at the top.
Session overrides remain memory-only; saved startup/environment credentials are
still used when an override is empty. A shared gateway key can serve multiple
features without entering it again; the card does not copy one service's session
override into another service.

Choosing **Custom / local server** for new speech or transcription configurations
uses `http://127.0.0.1:8000/v1/audio/speech` or
`http://127.0.0.1:8000/v1/audio/transcriptions`, with no hosted key environment
variable. Change the endpoint/model for your server under Advanced. The internal
OpenAI-compatible adapter names describe the wire format; these presets do not
select OpenAI's hosted API. Existing saved endpoints and key names are preserved.
Vercel and OpenRouter choices retain their gateway routes.

Changing the transcription service cancels the old operation and clears its
session key before applying the new service. Microphone/VAD preferences remain.
Saved environment credentials are resolved using the newly selected service.
Both the main model selector and advanced catalog controls use the active
transcription session key.


For Fish Audio, **Voice library → Browse Fish voices** sits next to the ordinary
Voice controls. Opening it makes no search request. Choose Public/My voices,
enter a title if wanted, then press Search (or Enter); extra tag/language/sort/page
size options are under More filters. Choose a result to update this character's
voice reference. The library uses the existing Fish credentials and invalidates
cached choices or pending results when its connection changes.

Advanced microphone tuning retains endpoint/exact-model overrides, keys,
language, VAD, silence timing, and manual diagnostics. Enablement, automatic send,
service and model selection stay on the ordinary Microphone card.

New microphone setups enable **Send when you stop talking**. Disable it to review transcripts before sending; recording always starts manually and never restarts itself. Existing saved choices remain in effect.
