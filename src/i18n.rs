//! The start screen in the chosen answer language: the bot's greeting and the suggestion cards.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::ui_text::{COLUMNS, TEXTS};

/// Column of [`TEXTS`] the menus are shown in (0 = English).
static UI_COLUMN: AtomicUsize = AtomicUsize::new(0);

/// Show the menus in `code` (a code from `LANGUAGES`; unknown codes mean English).
pub fn set_ui_language(code: &str) {
    UI_COLUMN.store(COLUMNS.iter().position(|c| *c == code).unwrap_or(0), Ordering::Relaxed);
}

/// A menu text in the current language; `english` is the text itself and the key.
/// Texts without a translation stay English.
pub fn t(english: &'static str) -> &'static str {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static INDEX: OnceLock<HashMap<&'static str, usize>> = OnceLock::new();
    let index = INDEX.get_or_init(|| TEXTS.iter().enumerate().map(|(i, row)| (row[0], i)).collect());
    match index.get(english) {
        Some(&row) => TEXTS[row][UI_COLUMN.load(Ordering::Relaxed)],
        None => english,
    }
}

/// [`t`] with `{0}`, `{1}`, … filled in.
pub fn tf(english: &'static str, args: &[&str]) -> String {
    let mut out = t(english).to_string();
    for (i, a) in args.iter().enumerate() {
        out = out.replace(&format!("{{{i}}}"), a);
    }
    out
}

/// The class bot's greeting as AIS.chat sends it.
pub const CLASS_GREETING: &str = "Hallo, wie kann ich Dir beim Coden helfen?";

/// Built-in translations of [`CLASS_GREETING`], by language code.
fn class_greeting(lang: &str) -> Option<&'static str> {
    Some(match lang {
        "de" => CLASS_GREETING,
        "en" => "Hello, how can I help you with coding?",
        "fr" => "Bonjour, comment puis-je t'aider à coder ?",
        "es" => "Hola, ¿cómo puedo ayudarte a programar?",
        "it" => "Ciao, come posso aiutarti a programmare?",
        "nl" => "Hallo, hoe kan ik je helpen met programmeren?",
        "pl" => "Cześć, jak mogę Ci pomóc w programowaniu?",
        "tr" => "Merhaba, kodlama konusunda sana nasıl yardımcı olabilirim?",
        "uk" => "Привіт, як я можу допомогти тобі з програмуванням?",
        "ru" => "Привет, чем я могу помочь тебе с программированием?",
        _ => return None,
    })
}

/// The greeting in `lang`, if it is known without asking the bot.
pub fn known_greeting(greeting: &str, lang: &str) -> Option<&'static str> {
    if greeting.trim() == CLASS_GREETING {
        class_greeting(lang)
    } else {
        None
    }
}

/// The four suggestion cards on the start screen (German when the language is automatic).
pub fn suggestions(lang: &str) -> [&'static str; 4] {
    match lang {
        "en" => ["Explain this code to me step by step", "Help me plan my app idea", "Find the bug in my program", "Write me a small Python game"],
        "fr" => [
            "Explique-moi ce code étape par étape",
            "Aide-moi à planifier mon idée d'appli",
            "Trouve l'erreur dans mon programme",
            "Écris-moi un petit jeu en Python",
        ],
        "es" => [
            "Explícame este código paso a paso",
            "Ayúdame a planificar mi idea de app",
            "Encuentra el error en mi programa",
            "Escríbeme un pequeño juego en Python",
        ],
        "it" => [
            "Spiegami questo codice passo dopo passo",
            "Aiutami a pianificare la mia idea di app",
            "Trova l'errore nel mio programma",
            "Scrivimi un piccolo gioco in Python",
        ],
        "nl" => [
            "Leg deze code stap voor stap uit",
            "Help me mijn app-idee te plannen",
            "Vind de fout in mijn programma",
            "Schrijf een klein Python-spel voor me",
        ],
        "pl" => [
            "Wyjaśnij mi ten kod krok po kroku",
            "Pomóż mi zaplanować pomysł na aplikację",
            "Znajdź błąd w moim programie",
            "Napisz mi małą grę w Pythonie",
        ],
        "tr" => [
            "Bu kodu bana adım adım açıkla",
            "Uygulama fikrimi planlamama yardım et",
            "Programımdaki hatayı bul",
            "Bana küçük bir Python oyunu yaz",
        ],
        "uk" => [
            "Поясни мені цей код крок за кроком",
            "Допоможи спланувати мою ідею застосунку",
            "Знайди помилку в моїй програмі",
            "Напиши мені маленьку гру на Python",
        ],
        "ru" => [
            "Объясни мне этот код шаг за шагом",
            "Помоги спланировать мою идею приложения",
            "Найди ошибку в моей программе",
            "Напиши мне небольшую игру на Python",
        ],
        _ => [
            "Erkläre mir diesen Code Schritt für Schritt",
            "Hilf mir, meine App-Idee zu planen",
            "Finde den Fehler in meinem Programm",
            "Schreib mir ein kleines Python-Spiel",
        ],
    }
}

/// The message that asks the bot to translate a greeting it does not know yet.
pub fn translation_request(greeting: &str, english_name: &str) -> String {
    format!(
        "Translate the following greeting into {english_name}. Reply with the translation only – no quotes, no \
         explanation, nothing else.\n\n{greeting}"
    )
}

/// Accept the bot's translation only if it looks like one: a short single text, not an essay or code.
pub fn clean_translation(reply: &str, original: &str) -> Option<String> {
    let t = reply.trim().trim_matches(|c| matches!(c, '"' | '„' | '“' | '”' | '«' | '»' | '\'')).trim();
    let max = original.chars().count() * 3 + 40;
    if t.is_empty() || t.chars().count() > max || t.contains("```") {
        return None;
    }
    Some(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_texts_are_complete() {
        let mut seen = std::collections::HashSet::new();
        for row in TEXTS {
            assert!(seen.insert(row[0]), "listed twice: {}", row[0]);
            let holes = (0..4).filter(|i| row[0].contains(&format!("{{{i}}}"))).count();
            for (col, text) in row.iter().enumerate() {
                assert!(!text.trim().is_empty(), "{} has no {}", row[0], COLUMNS[col]);
                let n = (0..4).filter(|i| text.contains(&format!("{{{i}}}"))).count();
                assert_eq!(n, holes, "placeholders of {:?} in {}", row[0], COLUMNS[col]);
            }
        }
        // every text the code asks for is in the table
        for file in [include_str!("app.rs"), include_str!("markdown.rs"), include_str!("conversations.rs")] {
            for call in ["t(\"", "tf(\""] {
                let mut rest = file;
                while let Some(at) = rest.find(call) {
                    let before = rest[..at].chars().last().unwrap_or(' ');
                    let after = &rest[at + call.len()..];
                    rest = after;
                    if before.is_alphanumeric() || before == '_' {
                        continue; // e.g. print(" or insert("
                    }
                    let key = after.split('"').next().unwrap().replace("\\n", "\n");
                    assert!(TEXTS.iter().any(|r| r[0] == key), "no translations for {key:?}");
                }
            }
        }
    }

    #[test]
    fn translates_and_fills_in() {
        set_ui_language("de");
        assert_eq!(t("New chat"), "Neuer Chat");
        assert_eq!(tf("Free via {0}", &["Groq"]), "Kostenlos über Groq");
        assert_eq!(t("not in the table"), "not in the table");
        set_ui_language("xx");
        assert_eq!(t("New chat"), "New chat");
    }
    use crate::settings::LANGUAGES;

    #[test]
    fn every_language_has_greeting_and_suggestions() {
        for (code, _, _) in LANGUAGES {
            assert!(known_greeting(CLASS_GREETING, code).is_some(), "{code}");
            assert!(suggestions(code).iter().all(|s| !s.is_empty()), "{code}");
        }
        assert_eq!(known_greeting(CLASS_GREETING, "en"), Some("Hello, how can I help you with coding?"));
        assert_eq!(known_greeting("Hi, I am a history bot.", "en"), None);
        assert_eq!(suggestions("")[0], "Erkläre mir diesen Code Schritt für Schritt");
    }

    #[test]
    fn accepts_only_plausible_translations() {
        assert_eq!(clean_translation(" \"Hi, I help with history!\" ", "Hallo, ich helfe bei Geschichte!").as_deref(), Some("Hi, I help with history!"));
        assert_eq!(clean_translation("", "Hallo"), None);
        assert_eq!(clean_translation(&"word ".repeat(100), "Hallo"), None);
        assert_eq!(clean_translation("```\nx\n```", "Hallo"), None);
    }
}
