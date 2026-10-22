use std::{io::Write as _, path::Path};

use atomic_write_file::{AtomicWriteFile, OpenOptions};

use easytier_core::management::{ConfigFileControl, ConfigFilePermission, ConfigFileStorage};

#[derive(Default)]
pub(crate) struct NativeConfigFileStorage;

#[async_trait::async_trait]
impl ConfigFileStorage for NativeConfigFileStorage {
    async fn inspect(&self, path: &Path) -> ConfigFileControl {
        let read_only = tokio::fs::metadata(path)
            .await
            .map(|metadata| metadata.permissions().readonly())
            .unwrap_or(true);
        ConfigFileControl::new(
            Some(path.to_owned()),
            if read_only {
                ConfigFilePermission::from(ConfigFilePermission::READ_ONLY)
            } else {
                ConfigFilePermission::default()
            },
        )
    }

    async fn read(&self, path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
        match tokio::fs::read(path).await {
            Ok(contents) => Ok(Some(contents)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    async fn write(&self, path: &Path, contents: &[u8]) -> anyhow::Result<()> {
        let path = path.to_owned();
        // Management serializes the portable network model. Keep the native
        // executable's settings in the same file across network updates.
        let contents = preserve_startup_settings(self.read(&path).await?.as_deref(), contents)?;
        tokio::task::spawn_blocking(move || {
            let mut options = OpenOptions::new();
            #[cfg(unix)]
            {
                atomic_write_file::unix::OpenOptionsExt::preserve_mode(&mut options, false);
                std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            }
            let mut file: AtomicWriteFile = options.open(path)?;
            file.write_all(&contents)?;
            file.commit()
        })
        .await??;
        Ok(())
    }

    async fn remove(&self, path: &Path) -> anyhow::Result<()> {
        tokio::fs::remove_file(path).await?;
        Ok(())
    }
}

fn preserve_startup_settings(previous: Option<&[u8]>, contents: &[u8]) -> anyhow::Result<Vec<u8>> {
    let Some(previous) = previous
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| toml::from_str::<toml::Table>(text).ok())
    else {
        return Ok(contents.to_owned());
    };
    let sections = ["core", "console_logger", "file_logger"];
    if !sections
        .iter()
        .any(|section| previous.contains_key(*section))
    {
        return Ok(contents.to_owned());
    }
    let mut updated: toml::Table = toml::from_str(std::str::from_utf8(contents)?)?;
    for section in sections {
        if let Some(value) = previous.get(section) {
            updated.insert(section.to_owned(), value.clone());
        }
    }
    Ok(toml::to_string_pretty(&updated)?.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn network_updates_preserve_native_startup_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let storage = NativeConfigFileStorage;
        storage.write(&path, b"hostname = 'old'\n[core]\ndaemon = true\n[file_logger]\nlevel = 'warn'\n[console_logger]\nlevel = 'info'\n").await.unwrap();
        storage.write(&path, b"hostname = 'new'\n").await.unwrap();
        let saved: toml::Table = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(saved["hostname"].as_str(), Some("new"));
        assert_eq!(saved["core"]["daemon"].as_bool(), Some(true));
        assert_eq!(saved["file_logger"]["level"].as_str(), Some("warn"));
        assert_eq!(saved["console_logger"]["level"].as_str(), Some("info"));
    }

    #[tokio::test]
    async fn config_write_is_atomic_and_private() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("instance.toml");
        let storage = NativeConfigFileStorage;

        storage.write(&path, b"first").await.unwrap();
        storage.write(&path, b"second").await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
