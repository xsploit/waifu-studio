mod color_correction;
pub mod data;
pub mod desktop_interaction;
pub mod desktop_occlusion;
pub mod desktop_platform;
mod durable_file;
pub mod inspect;
pub mod loading;
pub mod provider;
pub mod shaders;
#[cfg(windows)]
mod windows_process;
pub use inspect::{ModelInfo, VrmVersion, inspect_bytes, inspect_file};

pub mod animation;
pub mod attachments;
pub mod discord_audio;
pub mod listen;
pub mod silero;
pub mod voice;

pub mod mcp;
pub mod memory;

pub mod agents;
pub mod companion_tools;
pub mod delegation;

pub mod agent_loop;

pub mod environments;
pub mod voice_design;
pub mod voice_stream;

pub mod appearance;
pub mod behavior;
pub mod discord_gateway;
pub mod discord_host;
mod discord_identify;
pub use discord_identify::admission_for_worker as discord_identify_admission;
pub mod discord_messages;
pub mod discord_worker;
pub mod expressions;
pub mod extension_actions;
pub mod extension_audio;
pub mod extension_audio_job;
pub mod extension_connection;
pub mod extension_credentials;
pub mod extension_event_bridge;
pub mod extension_hooks;
pub mod extension_installed;
pub mod extension_lifecycle;
pub mod extension_manifest;
pub mod extension_playback_ledger;
pub mod extension_process;
pub mod extension_protocol;
pub mod extension_runtime;
pub mod extension_session;
pub mod extension_settings;
pub mod extension_speech_output;
pub mod extension_store;
pub mod extensions;
pub mod fish_live;
pub mod fish_ws;
pub mod gaze;
mod material_expressions;
pub mod metered_audio;
mod morph_sharing;
pub mod semantic_memory;
pub mod speech_pcm_routing;
pub mod speech_text;
pub mod spring_physics;
pub mod task_board;
pub mod twitch_auth;
pub mod twitch_eventsub;
pub mod twitch_worker;

pub mod lipsync;
pub mod viseme_audio;

pub mod bundled;
pub mod credentials;
pub mod evaluation;
pub mod facts_memory;
pub mod recipient_decision;
pub mod reply_affect;
pub mod reply_affect_controls;
pub mod scene_chat;
pub mod scene_roster;
pub mod voice_library;

pub mod anime_look;
pub mod desktop_perch;
pub mod desktop_pose;
pub mod rooms;
pub mod visuals;

pub mod model_catalog;
pub mod settings_kit;

pub mod vercel_speech;

pub mod reply_speech;

mod recall_terms;

pub mod routines;

pub mod appraisal;

pub mod reply_timing;

pub mod chat_decision;

/// Bounded native speech PCM conversion for future outgoing call transport.
pub mod call_pcm;

/// Device-independent single-decoder speech pacing.
pub mod speech_clock;
