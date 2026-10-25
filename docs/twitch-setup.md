# Twitch setup and acceptance

Current implementation checkpoint, 2026-10-22. The owned receiver uses EventSub, not IRC. Twitch currently recommends EventSub and its API for chat; IRC remains a limited alternative ([official chat guide](https://dev.twitch.tv/docs/chat)). A channel name alone does not configure this receiver: it listens to the account identified by the supplied user token.

## Connect the current receiver

1. Open **Extensions → Add… → Twitch**. The launcher requires Python 3; the worker is the native Waifu Studio executable. Process execution supports Windows and Linux.
2. Choose **Listen for** under Settings and save. The token must belong to the same registered application as the application ID. Use a registered public-client Twitch application with **Connect with Twitch**, or enter a compatible user token manually. Device-authorized credentials include private identity/expiry metadata for host-owned automatic refresh. Follow [Twitch token documentation](https://dev.twitch.tv/docs/authentication/getting-tokens-oauth/). An app-only token is insufficient for this receiver.
3. In **Twitch account**, enter your public-client application ID for first-time setup, then choose **Connect with Twitch**. For reconnecting, leave that field empty to reuse the saved application ID privately; a valid entered replacement takes precedence. An invalid replacement is rejected rather than silently using the saved ID. When the challenge arrives, choose **Open browser** and complete consent in Twitch. Keep the app open until authorization is saved. Manual fallback: expand **Manual tokens**, enter the application ID and user access token and choose **Save credentials**. Do not enter a client secret. Empty replacement fields preserve saved values. Credentials are separate from ordinary settings, stored with owner-only Unix permissions; the UI does not reveal saved values.
4. Open **Permissions** on the Twitch card and grant **Deliver external messages** for chat/context intake. Grant observation and motion playback for gestures, and expression control if you choose an Alert expression. These host permissions are separate from Twitch OAuth scopes. Load the intended character before enabling.
5. Turn on **Enabled** and inspect its status. **Advanced** contains process restart and diagnostics. Changing saved configuration or credentials restarts it. The receiver validates token identity and required scopes at startup and hourly.

| Listen for | Required Twitch scopes |
| --- | --- |
| Chat only | `user:read:chat` |
| Alerts only | `channel:read:subscriptions`, `bits:read` |
| Chat and alerts | All three scopes above |

The current implementation binds both broadcaster and chat reader to the token owner. There is no separate arbitrary-channel selector. Use a token for the intended broadcaster account. The app validates that the token’s application ID matches the saved application ID.

## Try reactions before a live event

Choose an Alert gesture and optional Alert expression, save, then open **Advanced → Test reaction** on an enabled installation with the required host grants and a loaded character. This separate child does not load credentials, use the network or deliver chat context. It uses saved values, so an unsaved selection is not tested. The expression overlaps the gesture after expression acceptance; motion completion is correlated to the real host terminal receipt.

A successful process result is not visual acceptance: watch the character. Disable the integration during another test to verify cleanup. The separate **Twitch replay (offline)** package additionally exercises attributed synthetic message delivery; restarting that package explicitly repeats its fixture.

## Live acceptance still required

With an authorized account, verify actual chat reception first, then an eligible subscription/cheer event when available. External text appears as attributed third-party context. To let accepted messages start replies, enable **Extensions → Incoming messages → Reply to incoming messages** for the loaded character. It uses that character’s configured chat and voice routes, preserves local drafts/attachments, and excludes external text from local-user memory extraction. The switch defaults off. Delivery acceptance does not guarantee a reply: busy/loading/configuration/cooldown failures keep saved context without retry. A running-app ten-second incoming-reply cooldown limits reaction loops; no extra host backlog is provided. Alerts delivered as context follow the same policy. This integration receives only and never posts Twitch chat.

Record event receipt, actual expression/gesture playback, return to talking/idle, disable during activity, character-switch cancellation and reconnect behavior separately. Confirm native capture/audio in a real recording. Existing offline tests do not establish any of these account or visible-playback outcomes.

Known bounds: one shared reaction cooldown is three seconds. Duplicate protection retains 256 envelope IDs for 330 seconds, including control messages; saturation stops the worker instead of evicting IDs and risking replay. The receiver retains up to16 pending/in-flight notification texts (at most4096 bytes each), FIFO, for15 seconds. Explicit Busy refusals of message delivery receive at most three attempts with at least one second between attempts; other errors, acceptance, missing ACKs, timeout and disconnect are never replayed. The original15-second deadline still applies to in-flight delivery, and host results arriving after either deadline cannot revive the reaction. Queue expiry/overflow is reported in bounded process logs. Target changes discard waiting and in-flight work. This is a short receiver backlog, not guaranteed delivery or a host reply queue. A lost connection can lose events; Twitch provides no replay during reconnection ([WebSocket guidance](https://dev.twitch.tv/docs/eventsub/handling-websocket-events/)). Restart resets the process cache and should not be presented as exactly-once delivery across restarts.

## Browser sign-in boundaries

The implemented device flow uses an explicit browser-open action, cancellable bounded background requests and the existing private credential store. One authorization worker runs at a time; cancellation retains its admission slot until it exits. Saved event selection determines scopes. Configuration or credential changes invalidate pending results; once the store starts its atomic write, cancellation cannot undo that write. Sign-in does not enable an integration or grant host permissions. Access and refresh tokens are saved atomically with private validated account/expiry metadata. For enabled Twitch installations, the host refreshes within five minutes of expiry and checks the returned account, application and required scopes before saving. The child never refreshes. A changed manual credential clears metadata and needs browser authorization before automatic refresh is available. A registered public client is required. An empty application-ID field reuses the saved value for this installation; saved credential values are never displayed or copied into UI drafts. Resolution runs on the store worker with current installation/credential revision checks. An explicit replacement is used only for that attempt and is saved together with tokens after successful authorization, not when Connect is pressed. No token or client secret should be requested in chat. Live authorization remains unverified.


## Automatic refresh boundaries

Refresh uses the same global authorization slot, cancellation and stale-install/credential checks as sign-in. It attempts each rotating refresh token at most once per credential revision in a host session, with at least60 seconds between automatic attempts globally. Unknown network or save outcomes are not automatically retried; use browser sign-in if refresh fails. Cancellation after Twitch consumes a refresh token can require sign-in again. Expiry metadata survives application restart; account identity is never inferred from the currently selected avatar.

Saving a successful rotation replaces the enabled receiver through the existing credential-revision mechanism. This can create an event gap and resets that child’s in-memory duplicate cache. Do not claim uninterrupted delivery or exactly-once actions across restarts. No live account/refresh acceptance has been performed; verification remains local fake HTTP and offline host/store tests.


## Chat chunks

**Chat intake → Chunks** is the new default; **Direct** keeps the earlier one-message context path. Chunks preserve the original Twitch message ID, viewer ID/name and exact text. The native receiver offers chat candidates to the host's four-second, bounded chunk buffer. Subscription/cheer alerts retain their immediate context and reaction path. Candidate acceptance is not history delivery, model generation or speech acceptance.

The host groups whole FIFO messages within the4096-byte context limit, waiting for active history/conversation work when possible; candidates expire after15 seconds and are discarded on avatar/source changes. Optional Pick chat highlights can choose up to two originals using the configured decision model; it runs only when incoming replies are also enabled. Choosing Chunks alone makes no decision-model calls.

### Picking chat highlights

Under **Extensions → Incoming messages**, enable replies, then optionally enable
**Pick chat highlights**. The adjacent **Decision model** dropdown reuses the same
connection and credentials as other decision features. Set Twitch's **Chat intake**
to **Chunks**; **Direct** bypasses selection.

The host asks for zero, one, or two offered messages per chunk. It saves only the
selected original text and author/source metadata, in original arrival order.
There are no generated quotes or summaries. Skipped messages are not replayed.
Unknown or duplicate choices, provider errors and timeouts discard that chunk
with a short notice; they do not fall back to answering every message. Choosing
none is normal and creates no conversation turn.

Selection runs off the render thread, with at most two outstanding requests
including cancelled workers. The request deadline is ten seconds; every original
still expires fifteen seconds after arrival. Turning off replies or highlight
selection, changing the decision connection/model, changing avatars, cancelling
a reply, or changing/revoking a source discards pending work. Blocking requests
may finish after cancellation, but their results cannot be applied.

Highlight selection is optional and starts off. When enabled, it sends candidate
chat text and author metadata to your configured decision provider. Existing
incoming-reply opt-in and memory-attribution boundaries still apply.
