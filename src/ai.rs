//! ZenDesktop :: ai.rs
//!
//! Cliente HTTP nativo para Ollama (IA local en 127.0.0.1:11434).
//! Permite clasificar archivos por semántica, listar modelos locales instalados
//! y reestructurar el escritorio desde cero creando nuevas cajas inteligentes.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct AiClient {
    pub host: String,
    pub port: u16,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct AiSuggestedRule {
    pub title: String,
    pub folder: String,
    pub color: String,
    pub extensions: Vec<String>,
}

impl Default for AiClient {
    fn default() -> Self {
        AiClient {
            host: String::from("127.0.0.1"),
            port: 11434,
            model: String::from("llama3.2"),
        }
    }
}

pub fn parse_host_port(url_or_host: &str, default_port: u16) -> (String, u16) {
    let clean = url_or_host
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    if let Some((h, p)) = clean.split_once(':') {
        let port = p.parse::<u16>().unwrap_or(default_port);
        (h.to_string(), port)
    } else {
        (clean.to_string(), default_port)
    }
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch => out.push(ch),
        }
    }
    out
}

impl AiClient {
    fn resolve_stream(&self, timeout: u64) -> Option<TcpStream> {
        let (host, port) = parse_host_port(&self.host, self.port);
        let addr_str = format!("{}:{}", host, port);
        
        let stream = if let Ok(sock) = addr_str.parse::<SocketAddr>() {
            TcpStream::connect_timeout(&sock, Duration::from_millis(timeout)).ok()?
        } else if let Ok(mut addrs) = addr_str.to_socket_addrs() {
            let sock = addrs.next()?;
            TcpStream::connect_timeout(&sock, Duration::from_millis(timeout)).ok()?
        } else {
            return None;
        };

        let _ = stream.set_read_timeout(Some(Duration::from_millis(timeout)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(timeout)));
        Some(stream)
    }

    /// Comprueba si Ollama responde en el host/puerto especificado.
    pub fn ping(&self) -> bool {
        let (host, port) = parse_host_port(&self.host, self.port);
        let Some(mut stream) = self.resolve_stream(500) else { return false; };

        let request = format!("GET /api/tags HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n\r\n", host, port);
        if stream.write_all(request.as_bytes()).is_ok() {
            let mut response = Vec::new();
            if stream.read_to_end(&mut response).is_ok() {
                return response.starts_with(b"HTTP/1.1 200 OK") || response.starts_with(b"HTTP/1.0 200 OK");
            }
        }
        false
    }

    /// Lista los nombres de todos los modelos montados/instalados en Ollama.
    pub fn list_models(&self) -> Vec<String> {
        let mut models = Vec::new();
        let (host, port) = parse_host_port(&self.host, self.port);
        let Some(mut stream) = self.resolve_stream(1000) else { return models; };

        let request = format!("GET /api/tags HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n\r\n", host, port);
        if stream.write_all(request.as_bytes()).is_ok() {
            let mut response = String::new();
            if stream.read_to_string(&mut response).is_ok() {
                let mut cursor = response.as_str();
                while let Some(idx) = cursor.find("\"name\":\"") {
                    let rest = &cursor[idx + 8..];
                    if let Some(end) = rest.find('"') {
                        let name = &rest[..end];
                        models.push(name.to_string());
                        cursor = &rest[end..];
                    } else {
                        break;
                    }
                }
            }
        }
        models
    }

    /// Analiza el conjunto de archivos del escritorio y sugiere nuevas cajas personalizadas desde cero.
    pub fn auto_cluster_desktop(&self, filenames: &[String]) -> Vec<AiSuggestedRule> {
        let mut rules = Vec::new();
        if filenames.is_empty() { return rules; }

        let file_sample = filenames.iter().take(30).cloned().collect::<Vec<_>>().join(", ");
        let prompt = format!(
            "Categorize these desktop files: [{}]. Create 3 to 5 organizer boxes. Output ONLY lines using format: Title | Folder | #HEXCOLOR | ext1, ext2",
            json_escape(&file_sample)
        );

        let json_body = format!(
            "{{\"model\": \"{}\", \"prompt\": \"{}\", \"stream\": false}}",
            json_escape(&self.model),
            prompt
        );

        let (host, port) = parse_host_port(&self.host, self.port);
        let Some(mut stream) = self.resolve_stream(8000) else { return rules; };

        let req = format!(
            "POST /api/generate HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            host, port, json_body.len(), json_body
        );

        if stream.write_all(req.as_bytes()).is_ok() {
            let mut raw = String::new();
            if stream.read_to_string(&mut raw).is_ok() {
                if let Some(body_start) = raw.find("\r\n\r\n") {
                    let body = &raw[body_start + 4..];
                    if let Some(resp_idx) = body.find("\"response\":\"") {
                        let after = &body[resp_idx + 12..];
                        if let Some(end_idx) = after.find('"') {
                            let text = after[..end_idx].replace("\\n", "\n").replace("\\\"", "\"");
                            for line in text.lines() {
                                let parts: Vec<&str> = line.split('|').collect();
                                if parts.len() >= 4 {
                                    let title = parts[0].trim().trim_start_matches('-').trim_start_matches('*').trim().to_string();
                                    let folder = parts[1].trim().to_string();
                                    let color = parts[2].trim().to_string();
                                    let exts: Vec<String> = parts[3]
                                        .split(',')
                                        .map(|s| s.trim().to_lowercase())
                                        .filter(|s| !s.is_empty())
                                        .collect();
                                    if !title.is_empty() && !folder.is_empty() {
                                        rules.push(AiSuggestedRule { title, folder, color, extensions: exts });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Fallback inteligente completo para cubrir accesos directos, carpetas y todos los tipos de archivo
        if rules.is_empty() {
            rules.push(AiSuggestedRule {
                title: "🔗 Accesos Directos".into(),
                folder: "Accesos Directos".into(),
                color: "#38BDF8".into(),
                extensions: vec!["lnk".into(), "url".into()],
            });
            rules.push(AiSuggestedRule {
                title: "📄 Documentos & Textos".into(),
                folder: "Documentos".into(),
                color: "#818CF8".into(),
                extensions: vec!["pdf".into(), "doc".into(), "docx".into(), "txt".into(), "xlsx".into(), "xls".into(), "pptx".into(), "csv".into(), "md".into()],
            });
            rules.push(AiSuggestedRule {
                title: "🖼️ Imágenes & Multimedia".into(),
                folder: "Media".into(),
                color: "#F472B6".into(),
                extensions: vec!["png".into(), "jpg".into(), "jpeg".into(), "gif".into(), "webp".into(), "svg".into(), "psd".into(), "mp4".into(), "mkv".into(), "avi".into(), "mp3".into(), "wav".into()],
            });
            rules.push(AiSuggestedRule {
                title: "⚙️ Aplicaciones & Ejecutables".into(),
                folder: "Instaladores".into(),
                color: "#FBBF24".into(),
                extensions: vec!["exe".into(), "msi".into(), "bat".into(), "cmd".into(), "ps1".into()],
            });
            rules.push(AiSuggestedRule {
                title: "📦 Comprimidos & Archivos".into(),
                folder: "Comprimidos".into(),
                color: "#FB923C".into(),
                extensions: vec!["zip".into(), "rar".into(), "7z".into(), "tar".into(), "gz".into(), "iso".into()],
            });
            rules.push(AiSuggestedRule {
                title: "💻 Código & Proyectos".into(),
                folder: "Proyectos".into(),
                color: "#34D399".into(),
                extensions: vec!["py".into(), "rs".into(), "js".into(), "ts".into(), "json".into(), "html".into(), "css".into(), "c".into(), "cpp".into()],
            });
            rules.push(AiSuggestedRule {
                title: "📁 Carpetas & Directorios".into(),
                folder: "Carpetas".into(),
                color: "#A78BFA".into(),
                extensions: vec![],
            });
            rules.push(AiSuggestedRule {
                title: "📂 Varios Archivos".into(),
                folder: "Varios".into(),
                color: "#94A3B8".into(),
                extensions: vec!["*".into()],
            });
        }

        rules
    }
}
