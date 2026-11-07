//! Where a request goes: the AIS.chat class bot (default) or a free model of an AI provider
//! (Groq, OpenRouter, Mistral, Gemini, …).
//! Everything that talks to "the bot" – chats, merging, file placement, greeting translation –
//! goes through here, so it works the same with either.

use anyhow::Result;

use crate::ais::{self, AttachmentData, ChatMessage, Event, Role};
use crate::settings::Settings;

/// For the free models: the class bot's own instructions (read from its AIS.chat page) make the
/// model behave like it; plus how answers are shown in the window.
pub fn system_prompt(s: &Settings) -> String {
    let mut p = String::new();
    if !s.character_instructions.trim().is_empty() {
        p.push_str(s.character_instructions.trim());
        p.push_str("\n\n");
    } else {
        p.push_str("You are a friendly programming assistant for school students. Explain clearly and give working code.\n\n");
    }
    p.push_str("Format answers in Markdown; put code in fenced code blocks with the language.");
    p
}

/// The conversation as `(role, content)` for an OpenAI-style API, with the app's hidden note
/// in front of the newest user message (as for AIS.chat).
pub fn openai_messages(s: &Settings, msgs: &[ChatMessage]) -> Vec<(String, String)> {
    let last_user = msgs.iter().rposition(|m| m.role == Role::User);
    let note = s.app_note();
    let mut out = vec![("system".to_string(), system_prompt(s))];
    for (i, m) in msgs.iter().enumerate() {
        let role = match m.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        let mut content = m.content.clone();
        // files uploaded to AIS.chat earlier in this chat cannot be read by another model
        let uploaded: Vec<&str> =
            m.attachments.iter().filter(|a| matches!(a.data, AttachmentData::Uploaded(_))).map(|a| a.name.as_str()).collect();
        if !uploaded.is_empty() {
            content.push_str(&format!("\n\n[Files attached in the class chat, not available here: {}]", uploaded.join(", ")));
        }
        if let (Some(n), true) = (&note, Some(i) == last_user) {
            content = format!("{n}\n\n{content}");
        }
        if role == "assistant" && content.trim().is_empty() {
            continue;
        }
        out.push((role.to_string(), content));
    }
    out
}

/// Send the conversation and stream the answer.
pub fn stream(s: &Settings, msgs: &[ChatMessage], session: &str, mut on_event: impl FnMut(&Event)) -> Result<()> {
    if s.uses_ai_models() {
        let (base, key) = (s.ai_base(), s.ai_key());
        let target = crate::models::Target { base: &base, key: &key, model: &s.model, provider: s.ai_provider().name };
        crate::models::stream_chat(&target, &openai_messages(s, msgs), |t| on_event(&Event::Text(t.to_string())))
    } else {
        ais::stream_reply(s, ais::build_body(s, msgs, session), on_event).map(|_| ())
    }
}

/// A one-off question that no chat sees (merging, file placement, translations), without the
/// answer-language and GitHub notes. Returns the whole answer.
pub fn ask(s: &Settings, question: &str) -> Result<String> {
    let mut plain = s.clone();
    plain.language.clear();
    plain.github_enabled = false;
    let msgs = vec![ChatMessage::user(question)];
    let mut reply = String::new();
    stream(&plain, &msgs, &ais::new_session_id(), |ev| {
        if let Event::Text(t) = ev {
            reply.push_str(t);
        }
    })?;
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ais::Attachment;

    #[test]
    fn builds_openai_messages() {
        let mut s = Settings { character_instructions: "ROLLE\nDu hilfst beim Coden.".into(), language: "en".into(), ..Default::default() };
        s.model = "llama-3.3-70b-versatile".into();
        let pdf = Attachment { name: "aufgabe.pdf".into(), size: 1, data: AttachmentData::Uploaded("f1".into()) };
        let msgs = vec![
            ChatMessage::greeting("Hallo!"),
            ChatMessage::user_with("Look at this", vec![pdf]),
            ChatMessage::assistant_placeholder(),
            ChatMessage::user("And now?"),
        ];
        let out = openai_messages(&s, &msgs);
        assert_eq!(out[0].0, "system");
        assert!(out[0].1.starts_with("ROLLE"));
        assert_eq!(out[1], ("assistant".to_string(), "Hallo!".to_string()));
        assert!(out[2].1.contains("not available here: aufgabe.pdf"));
        assert_eq!(out.len(), 4, "the empty answer is left out");
        assert!(out[3].1.starts_with("[Note from the chat app") && out[3].1.ends_with("And now?"));
    }
}
