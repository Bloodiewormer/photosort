//! The local review page. OCR text and file names are untrusted: everything goes through `esc`,
//! none of it is placed inside the script, and the page's policy blocks all network access.

use fsguard::DestPath;
use std::fmt::{self, Write as _};
use std::io::Write;

pub struct Line {
    pub rect: [f32; 4],
    pub text: String,
    pub score: f32,
    pub korean: bool,
}

pub struct Image {
    /// File name inside `thumbs/`.
    pub thumb: String,
    pub width: u32,
    pub height: u32,
    pub ms: f64,
    pub lines: Vec<Line>,
}

pub struct Entry {
    pub name: String,
    pub outcome: Result<Image, String>,
}

/// Counts and timings only: safe to print and to share.
pub struct Summary {
    provider: String,
    sampled: usize,
    read: usize,
    failed: usize,
    with_text: usize,
    lines: usize,
    korean: usize,
    median_ms: f64,
    p90_ms: f64,
}

impl Summary {
    pub fn of(entries: &[Entry], provider: String) -> Self {
        let images: Vec<&Image> = entries
            .iter()
            .filter_map(|e| e.outcome.as_ref().ok())
            .collect();
        let mut ms: Vec<f64> = images.iter().map(|i| i.ms).collect();
        ms.sort_by(f64::total_cmp);
        let at = |p| ocr::metrics::percentile(&ms, p);
        Self {
            provider,
            sampled: entries.len(),
            read: images.len(),
            failed: entries.len() - images.len(),
            with_text: images.iter().filter(|i| !i.lines.is_empty()).count(),
            lines: images.iter().map(|i| i.lines.len()).sum(),
            korean: images
                .iter()
                .flat_map(|i| &i.lines)
                .filter(|l| l.korean)
                .count(),
            median_ms: at(50.0),
            p90_ms: at(90.0),
        }
    }
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "proveedor {}; muestra {}; leídas {}; con error {}; con texto {}; líneas {} \
             (coreano {}); OCR por imagen: mediana {:.0} ms, p90 {:.0} ms",
            self.provider,
            self.sampled,
            self.read,
            self.failed,
            self.with_text,
            self.lines,
            self.korean,
            self.median_ms,
            self.p90_ms
        )
    }
}

pub fn esc(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&#39;".to_string(),
            c => c.to_string(),
        })
        .collect()
}

const HEAD: &str = r#"<!doctype html>
<html lang="es"><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src 'self' file:; style-src 'unsafe-inline'; script-src 'unsafe-inline'; base-uri 'none'; form-action 'none'">
<title>Revisión del piloto de OCR</title>
<style>
:root{color-scheme:light dark}
body{font:15px/1.45 system-ui,sans-serif;margin:0 auto;padding:16px;max-width:1500px}
section{display:grid;grid-template-columns:minmax(0,1fr) minmax(0,1fr);gap:16px;border-top:2px solid #8886;padding:16px 0}
h2{grid-column:1/-1;font-size:14px;margin:0;overflow-wrap:anywhere}
.pic{position:relative;align-self:start}.pic img{width:100%;display:block}
.pic svg{position:absolute;inset:0;width:100%;height:100%}
rect{fill:none;stroke:#e0218a;stroke-width:2;vector-effect:non-scaling-stroke}
.pic text{fill:#e0218a;stroke:#fff;paint-order:stroke;stroke-width:.2em;font-weight:700}
.line{display:flex;gap:8px;align-items:baseline;padding:3px 0;border-bottom:1px solid #8883}
.line b{min-width:2.5em}.line span{flex:1;overflow-wrap:anywhere;font-size:17px}
.low span{opacity:.6}
small{opacity:.75;white-space:nowrap}
textarea{width:100%;height:20em;font-family:monospace}
input[type=number]{width:5em}
@media(max-width:800px){section{grid-template-columns:1fr}}
</style></head><body>
"#;

const SCRIPT: &str = r#"<script>
const KEY = 'ocr-piloto-' + document.body.dataset.run;
const fields = [...document.querySelectorAll('[data-key]')];
let saved = {};
try { saved = JSON.parse(localStorage.getItem(KEY)) || {}; } catch (e) {}
for (const f of fields) if (f.dataset.key in saved) f.value = saved[f.dataset.key];
function save() {
  for (const f of fields) saved[f.dataset.key] = f.value;
  try { localStorage.setItem(KEY, JSON.stringify(saved)); } catch (e) {}
}
document.addEventListener('change', save);
document.addEventListener('click', event => {
  const all = event.target.closest('button[data-all]');
  if (!all) return;
  for (const s of all.closest('section').querySelectorAll('select'))
    if (!s.value) s.value = 'correcta';
  save();
});
function tally() { return {correcta: 0, parcial: 0, incorrecta: 0, inventada: 0, sin_marcar: 0}; }
document.getElementById('sum').addEventListener('click', () => {
  const total = tally(), recognizer = {}, score = {}, filtered = tally(), kept = tally();
  for (const s of document.querySelectorAll('select')) {
    const verdict = s.value || 'sin_marcar', d = s.closest('.line').dataset, value = Number(d.score);
    const bucket = value < 0.4 ? '<0.40' : value < 0.65 ? '0.40-0.65' : value < 0.8 ? '0.65-0.80'
      : value < 0.9 ? '0.80-0.90' : '>=0.90';
    total[verdict]++;
    (recognizer[d.recognizer] ||= tally())[verdict]++;
    (score[bucket] ||= tally())[verdict]++;
    (d.filtered === '1' ? filtered : kept)[verdict]++;
  }
  const missed = [...document.querySelectorAll('input[type=number]')].filter(i => i.value !== '');
  document.getElementById('result').value = JSON.stringify({
    ejecucion: document.getElementById('run').textContent,
    lineas: total,
    por_reconocedor: recognizer,
    por_puntuacion: score,
    bajo_el_filtro: filtered,
    sobre_el_filtro: kept,
    imagenes_con_perdidas_anotadas: missed.length,
    lineas_no_detectadas: missed.reduce((n, i) => n + Number(i.value), 0)
  }, null, 2);
});
</script></body></html>
"#;

pub fn render(entries: &[Entry], summary: &Summary, min_score: f32, seed: u64) -> String {
    let mut page = String::from(HEAD);
    // `write!` to a String cannot fail.
    let _ = writeln!(
        page,
        "<h1>Revisión del piloto de OCR</h1>\
         <p id=\"run\">{}</p>\
         <p>Este archivo y sus miniaturas contienen texto e imágenes de tus fotos: no los compartas. \
         Marca cada línea. Una línea <em>inventada</em> es texto donde no había nada escrito. \
         En cada imagen, anota cuántas líneas escritas no aparecen. \
         Las marcas se guardan en este navegador. No hace falta marcarlo todo. \
         Las líneas atenuadas tienen una puntuación menor que {min_score}.</p>",
        esc(&summary.to_string())
    );
    for (n, entry) in entries.iter().enumerate() {
        let _ = write!(page, "<section><h2>{}. {}</h2>", n + 1, esc(&entry.name));
        let image = match &entry.outcome {
            Ok(image) => image,
            Err(error) => {
                let _ = writeln!(page, "<p>No se pudo leer: {}</p></section>", esc(error));
                continue;
            }
        };
        let _ = write!(
            page,
            "<div class=\"pic\"><a href=\"thumbs/{src}\" target=\"_blank\">\
             <img loading=\"lazy\" src=\"thumbs/{src}\" width=\"{w}\" height=\"{h}\" \
             style=\"height:auto\" alt=\"imagen {number}\"></a>\
             <svg viewBox=\"0 0 {w} {h}\" style=\"pointer-events:none\" aria-hidden=\"true\">",
            src = esc(&image.thumb),
            w = image.width,
            h = image.height,
            number = n + 1
        );
        let font = (image.width.max(image.height) / 60).max(8);
        for (i, line) in image.lines.iter().enumerate() {
            let [x0, y0, x1, y1] = line.rect;
            let _ = write!(
                page,
                "<rect x=\"{x0}\" y=\"{y0}\" width=\"{}\" height=\"{}\"/>\
                 <text x=\"{x0}\" y=\"{y0}\" font-size=\"{font}\">{}</text>",
                (x1 - x0).max(0.0),
                (y1 - y0).max(0.0),
                i + 1
            );
        }
        page.push_str("</svg></div><div>");
        if image.lines.is_empty() {
            page.push_str("<p>Sin texto detectado.</p>");
        } else {
            page.push_str("<button type=\"button\" data-all>Lo no marcado es correcto</button>");
        }
        for (i, line) in image.lines.iter().enumerate() {
            let low = line.score < min_score;
            let model = if line.korean { "coreano" } else { "v6" };
            let _ = write!(
                page,
                "<div class=\"line{class}\" data-score=\"{score}\" data-recognizer=\"{model}\" \
                 data-filtered=\"{flag}\"><b>{number}</b><span>{text}</span>\
                 <small>{model} · {score:.2}</small>\
                 <select data-key=\"l{image}-{number}\" aria-label=\"Veredicto de la línea {number}\">\
                 <option value=\"\">—</option><option>correcta</option><option>parcial</option>\
                 <option>incorrecta</option><option>inventada</option></select></div>",
                class = if low { " low" } else { "" },
                score = line.score,
                flag = u8::from(low),
                number = i + 1,
                text = esc(&line.text),
                image = n + 1
            );
        }
        let _ = writeln!(
            page,
            "<p><label>Líneas escritas que no aparecen: \
             <input type=\"number\" min=\"0\" data-key=\"m{}\"></label></p></div></section>",
            n + 1
        );
    }
    page.push_str(
        "<h2>Resumen para compartir</h2>\
         <p>Solo contiene cantidades y tiempos: ni texto, ni nombres, ni rutas.</p>\
         <button type=\"button\" id=\"sum\">Generar resumen</button>\
         <textarea id=\"result\" readonly aria-label=\"Resumen\"></textarea>\n",
    );
    page.push_str(SCRIPT);
    // Marks are stored per run, so a second sample does not inherit the first one's.
    page.replacen(
        "<body>",
        &format!("<body data-run=\"{seed}-{}\">", entries.len()),
        1,
    )
}

#[allow(clippy::disallowed_methods)] // `create_new` on a `DestPath`: nothing is ever overwritten
pub fn write(
    path: &DestPath,
    entries: &[Entry],
    summary: &Summary,
    min_score: f32,
    seed: u64,
) -> Result<(), String> {
    let page = render(entries, summary, min_score, seed);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path.as_path())
        .and_then(|mut file| file.write_all(page.as_bytes()))
        .map_err(|e| format!("cannot write the report: {}", e.kind()))
}
