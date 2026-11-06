    pub fn load() -> Self {
        let mut s: Settings = Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        if !s.is_linked() {
            let link = if s.invite_link.trim().is_empty() { DEFAULT_INVITE_LINK.to_string() } else { s.invite_link.clone() };
            if s.apply_invite_link(&link).is_err() {
                let _ = s.apply_invite_link(DEFAULT_INVITE_LINK);
            }
        }
        if s.github_token.trim().is_empty() {
            s.github_token = crate::secret::load("github-token").unwrap_or_default();
        } else if s.save().is_ok() {
            // old plain-text token: now moved to the keystore and removed from the file
        }
        s
    }

    pub fn save(&self) -> Result<()> {
        let p = Self::path().ok_or_else(|| anyhow!("No user config folder on this system."))?;
        // keystore first: if it fails, the plain-text token stays in the file instead of being lost
        crate::secret::store("github-token", self.github_token.trim())?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("Could not create {}", dir.display()))?;
        }
        std::fs::write(&p, serde_json::to_string_pretty(self)?).with_context(|| format!("Could not write {}", p.display()))?;
        Ok(())
    }
