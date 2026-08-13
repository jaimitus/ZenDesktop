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
    pub name_patterns: Vec<String>,
}

/// Regla propuesta por el LLM a partir de una descripcion en lenguaje natural.
#[derive(Debug, Clone)]
pub struct AiRuleDraft {
    pub title: String,
    pub folder: String,
    pub extensions: Vec<String>,
    pub name_patterns: Vec<String>,
    pub regex: Option<String>,
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

    /// Envia una peticion POST con cuerpo JSON a `path` y devuelve el cuerpo
    /// de la respuesta (sin cabeceras).
    fn http_post(&self, path: &str, body: &str, timeout: u64) -> Option<String> {
        let (host, port) = parse_host_port(&self.host, self.port);
        let mut stream = self.resolve_stream(timeout)?;
        let req = format!(
            "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            path, host, port, body.len(), body
        );
        if stream.write_all(req.as_bytes()).is_err() {
            return None;
        }
        let mut raw = String::new();
        if stream.read_to_string(&mut raw).is_err() {
            return None;
        }
        let body_start = raw.find("\r\n\r\n")?;
        Some(raw[body_start + 4..].to_string())
    }

    /// Genera una respuesta con el modelo en modo JSON (`format: json`),
    /// devolviendo el texto JSON crudo generado por el modelo.
    fn generate_json(&self, prompt: &str) -> Option<String> {
        let body = format!(
            "{{\"model\":\"{}\",\"prompt\":\"{}\",\"stream\":false,\"format\":\"json\",\"options\":{{\"temperature\":0.2}}}}",
            json_escape(&self.model),
            json_escape(prompt)
        );
        let resp = self.http_post("/api/generate", &body, 30000)?;
        let v: serde_json::Value = serde_json::from_str(&resp).ok()?;
        v.get("response")?.as_str().map(|s| s.to_string())
    }

    /// Obtiene los vectores de embedding de una lista de textos (POST /api/embed).
    pub fn embed(&self, model: &str, inputs: &[String]) -> Vec<Vec<f32>> {
        if inputs.is_empty() {
            return Vec::new();
        }
        let arr = inputs
            .iter()
            .map(|s| format!("\"{}\"", json_escape(s)))
            .collect::<Vec<_>>()
            .join(",");
        let body = format!("{{\"model\":\"{}\",\"input\":[{}]}}", json_escape(model), arr);
        let Some(resp) = self.http_post("/api/embed", &body, 15000) else { return Vec::new(); };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&resp) else { return Vec::new(); };
        v.get("embeddings")
            .and_then(|e| e.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|e| {
                        e.as_array()
                            .map(|vec| {
                                vec.iter()
                                    .filter_map(|x| x.as_f64())
                                    .map(|x| x as f32)
                                    .collect()
                            })
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Pide al modelo que ponga titulo y carpeta a cada grupo de archivos.
    pub fn name_clusters(&self, clusters: &[Vec<String>], language: &str) -> Vec<(String, String)> {
        if clusters.is_empty() {
            return Vec::new();
        }
        let mut groups = String::new();
        for (i, names) in clusters.iter().enumerate() {
            groups.push_str(&format!("{}) {}\n", i + 1, names.join(", ")));
        }
        let prompt = format!(
            "You are organizing a Windows desktop. Below are groups of file names. Give each group a short title and a folder name. Respond in {lang}. Respond ONLY with JSON in this exact shape: {{\"clusters\":[{{\"title\":\"...\",\"folder\":\"...\"}}]}}.\nGroups:\n{groups}",
            lang = language_name(language),
            groups = groups,
        );
        let Some(json) = self.generate_json(&prompt) else { return Vec::new(); };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) else { return Vec::new(); };
        let mut out = Vec::new();
        if let Some(arr) = v.get("clusters").and_then(|c| c.as_array()) {
            for item in arr {
                let title = item.get("title").and_then(|t| t.as_str()).unwrap_or("").to_string();
                let folder = item.get("folder").and_then(|f| f.as_str()).unwrap_or("").to_string();
                if !title.is_empty() && !folder.is_empty() {
                    out.push((title, folder));
                }
            }
        }
        out
    }

    /// Convierte una descripcion en lenguaje natural en una regla concreta
    /// (titulo, carpeta, extensiones, patrones y regex).
    pub fn generate_rule_from_text(&self, description: &str, language: &str) -> Option<AiRuleDraft> {
        let prompt = format!(
            "You are an expert at writing file organization rules for a Windows desktop organizer. Convert the user's description into a filing rule. Respond in {lang}. Respond ONLY with JSON in this exact shape: {{\"title\":\"...\",\"folder\":\"...\",\"extensions\":[\"...\"],\"name_patterns\":[\"...\"],\"regex\":\"...\"}}. \"extensions\" are file extensions without the dot (e.g. \"pdf\"). \"name_patterns\" are glob patterns using * (e.g. \"factura*\"). \"regex\" matches the whole file name, or empty string if not needed. Use empty arrays/strings when a field does not apply.\nDescription: {description}",
            lang = language_name(language),
            description = description,
        );
        let json = self.generate_json(&prompt)?;
        let v: serde_json::Value = serde_json::from_str(&json).ok()?;
        let title = v.get("title").and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
        let folder = v.get("folder").and_then(|f| f.as_str()).unwrap_or("").trim().to_string();
        if title.is_empty() || folder.is_empty() {
            return None;
        }
        let extensions = v
            .get("extensions")
            .and_then(|e| e.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let name_patterns = v
            .get("name_patterns")
            .and_then(|e| e.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        // El regex se valida; si el modelo lo inventa mal, se descarta.
        let regex = v
            .get("regex")
            .and_then(|r| r.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .filter(|r| regex::Regex::new(r).is_ok());
        Some(AiRuleDraft {
            title,
            folder,
            extensions,
            name_patterns,
            regex,
        })
    }

    /// Analiza los archivos del escritorio usando embeddings + clustering
    /// aglomerativo, y usa el LLM solo para nombrar cada grupo.
    pub fn auto_cluster_desktop(&self, filenames: &[String], embed_model: &str, language: &str) -> Vec<AiSuggestedRule> {
        if filenames.is_empty() {
            return Vec::new();
        }
        // Muestreo acotado: suficiente para agrupar sin saturar el embedding.
        let sample: Vec<String> = filenames.iter().take(150).cloned().collect();
        let vectors = self.embed(embed_model, &sample);
        if vectors.len() != sample.len() {
            return fallback_rules();
        }

        let clusters = cluster_filenames(&vectors, 8);
        let mut name_lists: Vec<Vec<String>> = Vec::new();
        for c in &clusters {
            let names: Vec<String> = c.iter().filter_map(|&i| sample.get(i).cloned()).collect();
            if !names.is_empty() {
                name_lists.push(names);
            }
        }
        if name_lists.is_empty() {
            return fallback_rules();
        }

        let named = self.name_clusters(&name_lists, language);
        let mut rules = Vec::new();
        for (i, names) in name_lists.iter().enumerate() {
            let (title, folder) = named.get(i).cloned().unwrap_or_else(|| {
                let fallback = format!("Grupo {}", i + 1);
                (fallback.clone(), fallback)
            });
            let (extensions, name_patterns) = cluster_signature(names);
            if extensions.is_empty() && name_patterns.is_empty() {
                continue;
            }
            rules.push(AiSuggestedRule {
                title,
                folder,
                color: CLUSTER_COLORS[i % CLUSTER_COLORS.len()].to_string(),
                extensions,
                name_patterns,
            });
        }
        if rules.is_empty() { fallback_rules() } else { rules }
    }
}

// ---------------------------------------------------------------------------
// Clustering por similitud de embeddings
// ---------------------------------------------------------------------------

const CLUSTER_COLORS: [&str; 8] = [
    "#38BDF8", "#818CF8", "#F472B6", "#34D399", "#FBBF24", "#FB923C", "#A78BFA", "#94A3B8",
];

fn language_name(code: &str) -> &'static str {
    match code.trim().to_ascii_lowercase().as_str() {
        "es" => "Spanish",
        "de" => "German",
        "fr" => "French",
        "pt" => "Portuguese",
        "it" => "Italian",
        _ => "English",
    }
}

/// Similitud coseno entre dos vectores (1.0 = iguales, -1.0 = opuestos).
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..n {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    let denom = (na.sqrt() * nb.sqrt()).max(1e-9);
    dot / denom
}

/// Similitud media (average-linkage) entre dos clusters.
fn cluster_similarity(a: &[usize], b: &[usize], vectors: &[Vec<f32>]) -> f32 {
    let mut sum = 0.0f32;
    let mut count = 0u32;
    for &ia in a {
        for &ib in b {
            sum += cosine(&vectors[ia], &vectors[ib]);
            count += 1;
        }
    }
    if count == 0 { 0.0 } else { sum / count as f32 }
}

/// Agrupa vectores por similitud coseno (aglomerativo / average-linkage) hasta
/// quedarse con `max_clusters` grupos o bajar del umbral minimo de similitud.
fn cluster_filenames(vectors: &[Vec<f32>], max_clusters: usize) -> Vec<Vec<usize>> {
    let n = vectors.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![vec![0]];
    }
    let mut clusters: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    let max_clusters = max_clusters.max(1);
    while clusters.len() > max_clusters {
        let mut best = (f32::MIN, 0usize, 0usize);
        for i in 0..clusters.len() {
            for j in (i + 1)..clusters.len() {
                let sim = cluster_similarity(&clusters[i], &clusters[j], vectors);
                if sim > best.0 {
                    best = (sim, i, j);
                }
            }
        }
        if best.0 < 0.25 {
            break; // demasiado dispares: no fusionar
        }
        let (_, i, j) = best;
        let moved = clusters.remove(j);
        clusters[i].extend(moved);
    }
    clusters
}

/// Prefijo comun (sin extension ni numeros) de un grupo de nombres, como glob
/// `prefijo*`, o None si es demasiado corto.
fn common_prefix(names: &[String]) -> Option<String> {
    let bases: Vec<String> = names
        .iter()
        .map(|n| {
            let base = n.rsplit_once('.').map(|(b, _)| b).unwrap_or(n.as_str());
            base.trim_end_matches(|c: char| c.is_ascii_digit())
                .trim_end_matches(['-', '_', ' '])
                .to_ascii_lowercase()
        })
        .collect();
    let first = bases.first()?;
    let mut prefix = String::new();
    'outer: for (i, c) in first.chars().enumerate() {
        for b in bases.iter().skip(1) {
            match b.chars().nth(i) {
                Some(bc) if bc == c => {}
                _ => break 'outer,
            }
        }
        prefix.push(c);
    }
    if prefix.chars().count() >= 3 {
        Some(format!("{}*", prefix))
    } else {
        None
    }
}

/// Extrae la "firma" de un grupo: extensiones unicas y (si no hay extensiones)
/// un patron de prefijo comun.
fn cluster_signature(names: &[String]) -> (Vec<String>, Vec<String>) {
    let mut exts: Vec<String> = Vec::new();
    for n in names {
        if let Some(ext) = n.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()) {
            if !ext.is_empty() && ext.len() <= 8 && !exts.contains(&ext) {
                exts.push(ext);
            }
        }
    }
    exts.sort();
    exts.truncate(8);
    let mut patterns = Vec::new();
    if exts.is_empty() {
        if let Some(prefix) = common_prefix(names) {
            patterns.push(prefix);
        }
    }
    (exts, patterns)
}

/// Fallback deterministico por tipos de archivo cuando el LLM o los embeddings
/// no estan disponibles.
fn fallback_rules() -> Vec<AiSuggestedRule> {
    vec![
        AiSuggestedRule {
            title: "🔗 Accesos Directos".into(),
            folder: "Accesos Directos".into(),
            color: "#38BDF8".into(),
            extensions: vec!["lnk".into(), "url".into()],
            name_patterns: Vec::new(),
        },
        AiSuggestedRule {
            title: "📄 Documentos & Textos".into(),
            folder: "Documentos".into(),
            color: "#818CF8".into(),
            extensions: vec!["pdf".into(), "doc".into(), "docx".into(), "txt".into(), "xlsx".into(), "xls".into(), "pptx".into(), "csv".into(), "md".into()],
            name_patterns: Vec::new(),
        },
        AiSuggestedRule {
            title: "🖼️ Imágenes & Multimedia".into(),
            folder: "Media".into(),
            color: "#F472B6".into(),
            extensions: vec!["png".into(), "jpg".into(), "jpeg".into(), "gif".into(), "webp".into(), "svg".into(), "psd".into(), "mp4".into(), "mkv".into(), "avi".into(), "mp3".into(), "wav".into()],
            name_patterns: Vec::new(),
        },
        AiSuggestedRule {
            title: "⚙️ Aplicaciones & Ejecutables".into(),
            folder: "Instaladores".into(),
            color: "#FBBF24".into(),
            extensions: vec!["exe".into(), "msi".into(), "bat".into(), "cmd".into(), "ps1".into()],
            name_patterns: Vec::new(),
        },
        AiSuggestedRule {
            title: "📦 Comprimidos & Archivos".into(),
            folder: "Comprimidos".into(),
            color: "#FB923C".into(),
            extensions: vec!["zip".into(), "rar".into(), "7z".into(), "tar".into(), "gz".into(), "iso".into()],
            name_patterns: Vec::new(),
        },
        AiSuggestedRule {
            title: "💻 Código & Proyectos".into(),
            folder: "Proyectos".into(),
            color: "#34D399".into(),
            extensions: vec!["py".into(), "rs".into(), "js".into(), "ts".into(), "json".into(), "html".into(), "css".into(), "c".into(), "cpp".into()],
            name_patterns: Vec::new(),
        },
        AiSuggestedRule {
            title: "📁 Carpetas & Directorios".into(),
            folder: "Carpetas".into(),
            color: "#A78BFA".into(),
            extensions: vec![],
            name_patterns: Vec::new(),
        },
        AiSuggestedRule {
            title: "📂 Varios Archivos".into(),
            folder: "Varios".into(),
            color: "#94A3B8".into(),
            extensions: vec!["*".into()],
            name_patterns: Vec::new(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_measures_similarity() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![1.0f32, 0.0, 0.0];
        assert!((cosine(&a, &b) - 1.0).abs() < 1e-6);
        let c = vec![0.0f32, 1.0, 0.0];
        assert!(cosine(&a, &c).abs() < 1e-6);
    }

    #[test]
    fn clustering_groups_similar_vectors() {
        // Dos grupos claros: el eje X y el eje Y.
        let vectors = vec![
            vec![1.0f32, 0.0],
            vec![0.9f32, 0.1],
            vec![0.0f32, 1.0],
            vec![0.1f32, 0.9],
        ];
        let clusters = cluster_filenames(&vectors, 2);
        assert_eq!(clusters.len(), 2);
        for c in &clusters {
            assert_eq!(c.len(), 2);
        }
        // Cada cluster debe agrupar vectores del mismo eje.
        for c in &clusters {
            let a = c[0];
            let b = c[1];
            assert!(cosine(&vectors[a], &vectors[b]) > 0.9);
        }
    }

    #[test]
    fn cluster_signature_extracts_extensions_and_prefix() {
        let (exts, _) = cluster_signature(&["a.pdf".into(), "b.PDF".into(), "c.jpg".into()]);
        assert_eq!(exts, vec!["jpg", "pdf"]);

        let (exts2, patterns) = cluster_signature(&["factura-01".into(), "factura-02".into()]);
        assert!(exts2.is_empty());
        assert_eq!(patterns, vec!["factura*"]);
    }
}
