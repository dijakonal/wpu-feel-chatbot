    /// Personal access token. Kept in the OS keystore; only read from the file to migrate old versions.
    #[serde(skip_serializing)]
    pub github_token: String,
