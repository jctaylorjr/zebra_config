use serde::Serialize;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

// the contents of a USB mirror "Zebra" folder, which the printer would process in this order
#[derive(Default)]
pub struct Folder {
    pub firmware: Option<Vec<u8>>,        // appl/
    pub files: Vec<(String, Vec<u8>)>,    // files/, stored on E:
    pub commands: Vec<(String, Vec<u8>)>, // commands/, run in filename order
}

// what the GUI shows about the chosen folder, without reading the firmware
#[derive(Serialize)]
pub struct Summary {
    pub path: String,
    pub firmware: Option<String>,
    pub files: usize,
    pub commands: usize,
    pub checks: Vec<String>, // files that should be on E: afterwards
    pub error: Option<String>,
}

pub fn load(root: &Path) -> io::Result<Folder> {
    check(root)?;
    let mut appl = read_dir(&root.join("appl"))?;
    if appl.len() > 1 {
        return Err(io::Error::other("appl/ has more than one firmware file"));
    }
    Ok(Folder {
        firmware: appl.pop().map(|(_, data)| data),
        files: read_dir(&root.join("files"))?,
        commands: read_dir(&root.join("commands"))?,
    })
}

pub fn summary(root: &Path) -> Summary {
    let path = root.display().to_string();
    let result = check(root).and_then(|()| {
        let appl = list(&root.join("appl"))?;
        let files = list(&root.join("files"))?;
        let commands = read_dir(&root.join("commands"))?;
        if appl.len() > 1 {
            return Err(io::Error::other("appl/ has more than one firmware file"));
        }
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        Ok(Summary {
            path: path.clone(),
            firmware: appl.first().map(|(n, _)| n.clone()),
            files: files.len(),
            commands: commands.len(),
            checks: expected_files(&names, &commands),
            error: None,
        })
    });
    result.unwrap_or_else(|e| Summary {
        path,
        firmware: None,
        files: 0,
        commands: 0,
        checks: vec![],
        error: Some(e.to_string()),
    })
}

// what should be on E: once the folder is applied: its files/ plus anything a command file
// uploads, e.g. a PPME export's filename="E:WEBLINK1_CA.NRD"
pub fn expected_files(files: &[&str], commands: &[(String, Vec<u8>)]) -> Vec<String> {
    let mut out: Vec<String> = files.iter().map(|n| n.to_uppercase()).collect();
    for (_, data) in commands {
        for part in String::from_utf8_lossy(data).split("filename=\"").skip(1) {
            let name = part.split('"').next().unwrap_or("");
            if name.get(..2).is_some_and(|d| d.eq_ignore_ascii_case("E:")) {
                let name = name[2..].to_uppercase();
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
    }
    out
}

fn check(root: &Path) -> io::Result<()> {
    if !root.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "folder not found"));
    }
    if !["appl", "files", "commands"]
        .iter()
        .any(|s| root.join(s).is_dir())
    {
        return Err(io::Error::other(
            "no appl, files or commands folder in it; choose the Zebra folder itself",
        ));
    }
    Ok(())
}

// regular files sorted by name; a missing folder is just empty
fn list(dir: &Path) -> io::Result<Vec<(String, PathBuf)>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut out = vec![];
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            out.push((
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            ));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn read_dir(dir: &Path) -> io::Result<Vec<(String, Vec<u8>)>> {
    list(dir)?
        .into_iter()
        .map(|(name, path)| Ok((name, fs::read(path)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_counts_a_zebra_folder_and_rejects_its_parent() {
        let root = std::env::temp_dir().join(format!("zebra_usb_test_{}", std::process::id()));
        let zebra = root.join("Zebra");
        for (sub, name) in [
            ("appl", "V1.zpl"),
            ("files", "a.get"),
            ("commands", "1.zpl"),
            ("commands", "2.zpl"),
        ] {
            fs::create_dir_all(zebra.join(sub)).unwrap();
            fs::write(zebra.join(sub).join(name), "x").unwrap();
        }

        let s = summary(&zebra);
        assert_eq!(
            (s.firmware.as_deref(), s.files, s.commands, s.error),
            (Some("V1.zpl"), 1, 2, None)
        );
        assert!(
            summary(&root)
                .error
                .unwrap()
                .contains("choose the Zebra folder itself")
        );
        assert!(summary(&root.join("missing")).error.is_some());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn expected_files_include_uploads_to_e() {
        let ppme = b"{}--b\r\nContent-Disposition: form-data; name=\"files\"; filename=\"\\\\Certs\\\\CA.cer\"\r\n\r\nx\r\n--b\r\nContent-Disposition: form-data; name=\"files\"; filename=\"E:WEBLINK1_CA.NRD\"\r\n\r\nx\r\n--b--\r\n{}{\"device.reset\" : \"\"}";
        let commands = vec![("export.json".to_string(), ppme.to_vec())];
        assert_eq!(
            expected_files(&["feedback.get"], &commands),
            vec!["FEEDBACK.GET", "WEBLINK1_CA.NRD"]
        );
    }
}
