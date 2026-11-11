# The school login – what happens with your password

*Deutsch: [weiter unten](#die-schul-anmeldung--was-mit-deinem-passwort-passiert).*

Before you can use the chat, the app asks for your **Moodle user name and password** of the
school. It checks them at the school's Moodle, **https://moodle.ohg-goe.net**, exactly like
logging in on the Moodle website.

## In short

| | |
|---|---|
| Where is the password sent? | **Only** to `https://moodle.ohg-goe.net/login/index.php`, the school's own login page. |
| Is the connection encrypted? | **Yes**, always (https). The code refuses plain http. |
| Is the password saved? | **No.** Not in a file, not in the settings, not in the password store, not in a log. |
| Does anyone else get it? | **No.** Not the chat bot, not AIS.chat, not GitHub, not the sync server, not the developers. |
| What does the app learn from Moodle? | Only **"login worked"** or **"login did not work"**. It reads no courses, grades, messages or names. |
| Does a Moodle session stay open? | **No.** The app logs out of Moodle again right after the check. |
| What is saved? | Only your **user name**, so you don't have to type it again next time. |
| Do I have to log in every time? | Yes, every time the app starts, because the password is not saved. |

## Step by step

This is what the app does when you press **Log in**. A browser does the same steps when you
log in on the Moodle website.

1. **Open the login page.** The app loads `https://moodle.ohg-goe.net/login/index.php`. Moodle
   sends back the page and a *session cookie*: a random number so that Moodle recognises the
   next request. The page contains a hidden one-time code, the *login token*, which Moodle
   wants back together with the form.
2. **Send the form.** The app sends your user name, your password and the login token to the
   same login page. This is the only time your password leaves your computer.
3. **Follow Moodle's forwarding.** After a login, Moodle forwards ("redirects") to another
   page. The app follows it, but **only to pages of moodle.ohg-goe.net**. If Moodle ever
   pointed somewhere else, the app would stop. The password is not sent again here, only the
   session cookie.
4. **Look for the "Log out" link.** A page for logged-in users has a "Log out" link. If it is
   there, the login worked. If not, the user name or password was wrong.
5. **Log out.** The app opens the log-out link, so the Moodle session ends right away.

Afterwards the password is forgotten. The text box is emptied as soon as you press **Log in**,
and the check throws its copy away when it is done.

## Check it yourself

The whole check is in one file: [`src/moodle_login.rs`](../src/moodle_login.rs). It is about
200 lines without the tests, more than half of them comments, and every step is explained in
plain words. You don't need to
know Rust to follow the comments.

Where the password appears in the code:

| file | place | what happens |
|---|---|---|
| `src/app.rs` | field `school_login_password` | holds what you type into the password box |
| `src/app.rs` | function `start_school_login` | takes the password out of the box (the box is empty again) and hands it to the check |
| `src/moodle_login.rs` | function `check_login` → `try_login` | puts it into the login form for moodle.ohg-goe.net (step 2) |

That's all. You can confirm it by searching the code for `school_login_password` and for
`check_login`. The address that is actually contacted is written in `src/moodle_login.rs`
(`MOODLE_ADDRESS` and `LOGIN_PAGE`). Elsewhere `moodle.ohg-goe.net` only appears in texts shown
on the screen (and their translations in `src/ui_text.rs`).

A test in the same file (`talks_only_to_the_school_moodle_over_https`) checks that the address
is https and that the password goes into exactly one form. Another test (`real_moodle`) tries a
made-up account against the real Moodle and expects "not accepted":

```bash
cargo test -- --ignored real_moodle
```

## Limits (honestly)

- This is a **check in the app on your computer**. Someone who changes the app's source code
  and builds it themselves could remove it. It keeps out people without a school account; it is
  not a lock that cannot be broken.
- If the school changes how its Moodle login page works, the check may stop working. The app
  then says that Moodle answered in an unexpected way.
- Moodle itself records logins, as it does for every login on the website.

---

# Die Schul-Anmeldung – was mit deinem Passwort passiert

Bevor du den Chat benutzen kannst, fragt die App nach deinem **Moodle-Benutzernamen und
-Passwort** der Schule. Sie prüft beides beim Moodle der Schule, **https://moodle.ohg-goe.net**,
genau wie beim Anmelden auf der Moodle-Webseite.

## Kurz gesagt

| | |
|---|---|
| Wohin wird das Passwort geschickt? | **Nur** an `https://moodle.ohg-goe.net/login/index.php`, die Anmeldeseite der Schule. |
| Ist die Verbindung verschlüsselt? | **Ja**, immer (https). Unverschlüsseltes http lehnt der Code ab. |
| Wird das Passwort gespeichert? | **Nein.** Nicht in einer Datei, nicht in den Einstellungen, nicht im Passwortspeicher, nicht in einem Protokoll. |
| Bekommt es sonst jemand? | **Nein.** Nicht der Chatbot, nicht AIS.chat, nicht GitHub, nicht der Sync-Server, nicht die Entwickler. |
| Was erfährt die App von Moodle? | Nur **„Anmeldung hat geklappt“** oder **„hat nicht geklappt“**. Sie liest keine Kurse, Noten, Nachrichten oder Namen. |
| Bleibt eine Moodle-Sitzung offen? | **Nein.** Die App meldet sich direkt nach der Prüfung wieder bei Moodle ab. |
| Was wird gespeichert? | Nur dein **Benutzername**, damit du ihn nächstes Mal nicht neu tippen musst. |
| Muss ich mich jedes Mal anmelden? | Ja, bei jedem Start der App, weil das Passwort nicht gespeichert wird. |

## Schritt für Schritt

Das macht die App, wenn du auf **Anmelden** drückst. Ein Browser macht dieselben Schritte,
wenn du dich auf der Moodle-Webseite anmeldest.

1. **Anmeldeseite öffnen.** Die App lädt `https://moodle.ohg-goe.net/login/index.php`. Moodle
   schickt die Seite und ein *Sitzungs-Cookie* zurück: eine Zufallszahl, an der Moodle die
   nächste Anfrage wiedererkennt. In der Seite steht ein versteckter Einmal-Code, der
   *Login-Token*, den Moodle zusammen mit dem Formular zurückhaben will.
2. **Formular abschicken.** Die App schickt Benutzername, Passwort und Login-Token an dieselbe
   Anmeldeseite. Nur in diesem Moment verlässt dein Passwort deinen Computer.
3. **Moodles Weiterleitung folgen.** Nach einer Anmeldung leitet Moodle auf eine andere Seite
   weiter. Die App folgt, aber **nur zu Seiten von moodle.ohg-goe.net**. Würde Moodle
   woanders hin zeigen, bricht die App ab. Das Passwort wird dabei nicht noch einmal geschickt,
   nur das Sitzungs-Cookie.
4. **Nach dem „Abmelden“-Link schauen.** Eine Seite für angemeldete Nutzer hat einen
   „Abmelden“-Link. Ist er da, hat die Anmeldung geklappt. Sonst waren Benutzername oder
   Passwort falsch.
5. **Abmelden.** Die App ruft den Abmelde-Link auf, damit die Moodle-Sitzung sofort endet.

Danach ist das Passwort vergessen. Das Textfeld wird geleert, sobald du auf **Anmelden**
drückst, und die Prüfung wirft ihre Kopie weg, wenn sie fertig ist.

## Selbst nachprüfen

Die ganze Prüfung steht in einer Datei: [`src/moodle_login.rs`](../src/moodle_login.rs). Sie
hat ohne Tests etwa 200 Zeilen, mehr als die Hälfte davon Kommentare, und jeder Schritt ist in
einfachen Worten erklärt (auf Englisch). Rust musst du dafür nicht können.

Wo das Passwort im Code vorkommt:

| Datei | Stelle | Was passiert |
|---|---|---|
| `src/app.rs` | Feld `school_login_password` | hält, was du ins Passwortfeld tippst |
| `src/app.rs` | Funktion `start_school_login` | nimmt das Passwort aus dem Feld (das Feld ist wieder leer) und gibt es an die Prüfung |
| `src/moodle_login.rs` | Funktion `check_login` → `try_login` | setzt es in das Anmeldeformular für moodle.ohg-goe.net (Schritt 2) |

Mehr nicht. Du kannst das nachprüfen, indem du im Code nach `school_login_password` und
`check_login` suchst. Die Adresse, die tatsächlich aufgerufen wird, steht in `src/moodle_login.rs`
(`MOODLE_ADDRESS` und `LOGIN_PAGE`). Sonst kommt `moodle.ohg-goe.net` nur in Texten vor, die auf
dem Bildschirm angezeigt werden (und deren Übersetzungen in `src/ui_text.rs`).

## Grenzen (ehrlich gesagt)

- Das ist eine **Prüfung in der App auf deinem Computer**. Wer den Quellcode ändert und die App
  selbst baut, könnte sie entfernen. Sie hält Leute ohne Schulkonto fern, ist aber kein
  unknackbares Schloss.
- Wenn die Schule ihre Moodle-Anmeldeseite umbaut, kann die Prüfung aufhören zu funktionieren.
  Die App sagt dann, dass Moodle unerwartet geantwortet hat.
- Moodle selbst protokolliert Anmeldungen, so wie bei jeder Anmeldung auf der Webseite.
