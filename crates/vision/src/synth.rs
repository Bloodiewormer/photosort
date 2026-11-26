//! Synthetic images for verifying the vision pipeline (preprocessing, tokenization, inference)
//! without reading any user photos or external datasets.
//!
//! Passing retrieval on these synthetic images proves the pipeline is wired correctly; it says
//! NOTHING about accuracy on real photos, real manga, or real faces.

use image::{Rgb, RgbImage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    ShapeColor,
    SolidColor,
    SceneMock,
}

pub struct SynthItem {
    pub id: &'static str,
    pub group: Group,
    pub image: RgbImage,
    pub prompt_en: &'static str,
    pub prompt_es: &'static str,
}

/// Generates the full synthetic verification set:
/// - 12 shape × color combinations (4 colors × 3 shapes)
/// - 4 solid color images
/// - 4 simple scene mocks (grayscale comic panels with speech bubbles, text document page,
///   phone chat screenshot mock, smooth photo-like gradient landscape)
pub fn verification_set() -> Vec<SynthItem> {
    let mut items = Vec::with_capacity(20);

    // 1. Shape × color grid (4 × 3 = 12)
    let colors: [(&str, &str, &str, Rgb<u8>); 4] = [
        ("red", "red", "rojo", Rgb([220, 35, 35])),
        ("blue", "blue", "azul", Rgb([35, 85, 225])),
        ("green", "green", "verde", Rgb([35, 175, 55])),
        ("yellow", "yellow", "amarillo", Rgb([235, 200, 25])),
    ];
    let shapes: [(&str, &str, &str); 3] = [
        ("circle", "circle", "círculo"),
        ("square", "square", "cuadrado"),
        ("triangle", "triangle", "triángulo"),
    ];
    let prompts: &[(&str, &str, &str)] = &[
        (
            "red_circle",
            "a red circle on a white background",
            "un círculo rojo sobre fondo blanco",
        ),
        (
            "red_square",
            "a red square on a white background",
            "un cuadrado rojo sobre fondo blanco",
        ),
        (
            "red_triangle",
            "a red triangle on a white background",
            "un triángulo rojo sobre fondo blanco",
        ),
        (
            "blue_circle",
            "a blue circle on a white background",
            "un círculo azul sobre fondo blanco",
        ),
        (
            "blue_square",
            "a blue square on a white background",
            "un cuadrado azul sobre fondo blanco",
        ),
        (
            "blue_triangle",
            "a blue triangle on a white background",
            "un triángulo azul sobre fondo blanco",
        ),
        (
            "green_circle",
            "a green circle on a white background",
            "un círculo verde sobre fondo blanco",
        ),
        (
            "green_square",
            "a green square on a white background",
            "un cuadrado verde sobre fondo blanco",
        ),
        (
            "green_triangle",
            "a green triangle on a white background",
            "un triángulo verde sobre fondo blanco",
        ),
        (
            "yellow_circle",
            "a yellow circle on a white background",
            "un círculo amarillo sobre fondo blanco",
        ),
        (
            "yellow_square",
            "a yellow square on a white background",
            "un cuadrado amarillo sobre fondo blanco",
        ),
        (
            "yellow_triangle",
            "a yellow triangle on a white background",
            "un triángulo amarillo sobre fondo blanco",
        ),
    ];
    let mut p_idx = 0;
    for (_, _, _, rgb) in colors {
        for (shape_id, _, _) in shapes {
            let (id, en, es) = prompts[p_idx];
            p_idx += 1;
            let image = render_shape(shape_id, rgb);
            items.push(SynthItem {
                id,
                group: Group::ShapeColor,
                image,
                prompt_en: en,
                prompt_es: es,
            });
        }
    }

    // 2. Solid colors (4)
    for (id, rgb, en, es) in [
        (
            "solid_red",
            Rgb([220, 30, 30]),
            "a solid red image",
            "una imagen completamente roja",
        ),
        (
            "solid_blue",
            Rgb([30, 75, 220]),
            "a solid blue image",
            "una imagen completamente azul",
        ),
        (
            "solid_green",
            Rgb([30, 175, 50]),
            "a solid green image",
            "una imagen completamente verde",
        ),
        (
            "solid_black",
            Rgb([10, 10, 10]),
            "a solid black image",
            "una imagen completamente negra",
        ),
    ] {
        items.push(SynthItem {
            id,
            group: Group::SolidColor,
            image: RgbImage::from_pixel(320, 320, rgb),
            prompt_en: en,
            prompt_es: es,
        });
    }

    // 3. Scene mocks (4)
    items.push(SynthItem {
        id: "scene_comic",
        group: Group::SceneMock,
        image: render_comic_panels(),
        prompt_en: "black and white comic book page with panels and speech bubbles",
        prompt_es: "página de cómic en blanco y negro con viñetas y bocadillos de diálogo",
    });
    items.push(SynthItem {
        id: "scene_document",
        group: Group::SceneMock,
        image: render_text_document(),
        prompt_en: "a white page of printed text document",
        prompt_es: "una página blanca de un documento de texto impreso",
    });
    items.push(SynthItem {
        id: "scene_phone_chat",
        group: Group::SceneMock,
        image: render_phone_chat(),
        prompt_en: "a smartphone screenshot of a chat conversation with message bubbles",
        prompt_es: "una captura de pantalla de teléfono de una conversación de chat con mensajes",
    });
    items.push(SynthItem {
        id: "scene_landscape",
        group: Group::SceneMock,
        image: render_gradient_landscape(),
        prompt_en: "a scenic landscape with mountains, blue sky, sun and a lake",
        prompt_es: "un paisaje con montañas, cielo azul, sol y un lago",
    });

    items
}

/// Generates `count` larger synthetic images (typical photo/page resolutions) for benchmarking
/// preprocessing + inference separately.
pub fn bench_images(count: usize) -> Vec<RgbImage> {
    let base = [
        render_gradient_landscape(),
        render_comic_panels(),
        render_text_document(),
        render_phone_chat(),
    ];
    (0..count)
        .map(|i| {
            let src = &base[i % base.len()];
            // Vary resolution slightly so resize does realistic work (around 960x720 / 720x1280).
            let mut img = src.clone();
            let shift = ((i * 7) % 15) as u8;
            for px in img.pixels_mut().step_by(97) {
                px[0] = px[0].saturating_add(shift);
            }
            img
        })
        .collect()
}

fn fill_rect(img: &mut RgbImage, x0: u32, y0: u32, x1: u32, y1: u32, color: Rgb<u8>) {
    let x1 = x1.min(img.width());
    let y1 = y1.min(img.height());
    for y in y0..y1 {
        for x in x0..x1 {
            img.put_pixel(x, y, color);
        }
    }
}

fn stroke_rect(img: &mut RgbImage, x0: u32, y0: u32, x1: u32, y1: u32, t: u32, color: Rgb<u8>) {
    fill_rect(img, x0, y0, x1, y0 + t, color);
    fill_rect(img, x0, y1.saturating_sub(t), x1, y1, color);
    fill_rect(img, x0, y0, x0 + t, y1, color);
    fill_rect(img, x1.saturating_sub(t), y0, x1, y1, color);
}

fn fill_ellipse(img: &mut RgbImage, cx: f32, cy: f32, rx: f32, ry: f32, color: Rgb<u8>) {
    let x0 = (cx - rx).max(0.0) as u32;
    let y0 = (cy - ry).max(0.0) as u32;
    let x1 = ((cx + rx).ceil() as u32).min(img.width());
    let y1 = ((cy + ry).ceil() as u32).min(img.height());
    for y in y0..y1 {
        let dy = (y as f32 + 0.5 - cy) / ry;
        for x in x0..x1 {
            let dx = (x as f32 + 0.5 - cx) / rx;
            if dx * dx + dy * dy <= 1.0 {
                img.put_pixel(x, y, color);
            }
        }
    }
}

fn stroke_ellipse(
    img: &mut RgbImage,
    center: (f32, f32),
    radii: (f32, f32),
    thickness: f32,
    fill: Rgb<u8>,
    border: Rgb<u8>,
) {
    let (cx, cy) = center;
    let (rx, ry) = radii;
    fill_ellipse(img, cx, cy, rx, ry, border);
    fill_ellipse(
        img,
        cx,
        cy,
        (rx - thickness).max(1.0),
        (ry - thickness).max(1.0),
        fill,
    );
}

fn fill_rounded_rect(
    img: &mut RgbImage,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    r: u32,
    color: Rgb<u8>,
) {
    let r = r.min((x1 - x0) / 2).min((y1 - y0) / 2);
    fill_rect(img, x0 + r, y0, x1 - r, y1, color);
    fill_rect(img, x0, y0 + r, x1, y1 - r, color);
    let rf = r as f32;
    for (cx, cy) in [
        ((x0 + r) as f32, (y0 + r) as f32),
        ((x1 - r) as f32, (y0 + r) as f32),
        ((x0 + r) as f32, (y1 - r) as f32),
        ((x1 - r) as f32, (y1 - r) as f32),
    ] {
        fill_ellipse(img, cx, cy, rf, rf, color);
    }
}

fn render_shape(shape: &str, color: Rgb<u8>) -> RgbImage {
    let mut img = RgbImage::from_pixel(320, 320, Rgb([250, 250, 250]));
    match shape {
        "circle" => fill_ellipse(&mut img, 160.0, 160.0, 95.0, 95.0, color),
        "square" => fill_rect(&mut img, 70, 70, 250, 250, color),
        "triangle" => {
            for y in 65..255 {
                let t = (y - 65) as f32 / 190.0;
                let half = (t * 105.0) as u32;
                let x0 = 160u32.saturating_sub(half);
                let x1 = (160 + half).min(320);
                for x in x0..x1 {
                    img.put_pixel(x, y, color);
                }
            }
        }
        _ => {}
    }
    img
}

/// Grayscale comic page mock with 4 bordered panels, halftone shading, and speech bubbles.
fn render_comic_panels() -> RgbImage {
    let (w, h) = (720, 960);
    let mut img = RgbImage::from_pixel(w, h, Rgb([245, 245, 245]));
    let black = Rgb([18, 18, 18]);
    let white = Rgb([252, 252, 252]);

    let panels = [
        (30, 30, 345, 450),
        (375, 30, 690, 450),
        (30, 480, 690, 700),
        (30, 730, 690, 930),
    ];
    for (i, &(x0, y0, x1, y1)) in panels.iter().enumerate() {
        // Screentone / grayscale panel background
        for y in y0..y1 {
            for x in x0..x1 {
                let dot = if (x % 6 < 2) && (y % 6 < 2) { 170 } else { 222 };
                let shade = if i == 2 && y > (y0 + y1) / 2 {
                    dot - 60
                } else {
                    dot
                };
                img.put_pixel(x, y, Rgb([shade, shade, shade]));
            }
        }
        // Simple dark silhouette figure inside each panel
        let cx = ((x0 + x1) / 2) as f32;
        let cy = (y1 - 70) as f32;
        fill_ellipse(&mut img, cx, cy - 55.0, 28.0, 34.0, Rgb([55, 55, 55]));
        fill_ellipse(&mut img, cx, cy + 25.0, 52.0, 65.0, Rgb([40, 40, 40]));
        stroke_rect(&mut img, x0, y0, x1, y1, 6, black);
    }

    // Speech bubbles with fake text lines inside
    for (cx, cy, rx, ry) in [
        (135.0, 120.0, 82.0, 48.0),
        (560.0, 125.0, 95.0, 52.0),
        (165.0, 555.0, 100.0, 46.0),
        (535.0, 805.0, 110.0, 48.0),
    ] {
        stroke_ellipse(&mut img, (cx, cy), (rx, ry), 4.0, white, black);
        // Speech tail
        fill_rect(
            &mut img,
            (cx - 8.0) as u32,
            (cy + ry - 4.0) as u32,
            (cx + 8.0) as u32,
            (cy + ry + 16.0) as u32,
            black,
        );
        // Horizontal text strokes inside bubble
        for row in 0..3 {
            let y = (cy - 16.0 + row as f32 * 14.0) as u32;
            let x0 = (cx - rx * 0.55) as u32;
            let x1 = (cx + rx * (0.55 - row as f32 * 0.1)) as u32;
            fill_rect(&mut img, x0, y, x1, y + 5, black);
        }
    }
    img
}

/// White page with dark header and simulated paragraphs of printed text.
fn render_text_document() -> RgbImage {
    let (w, h) = (720, 960);
    let mut img = RgbImage::from_pixel(w, h, Rgb([252, 252, 250]));
    let ink = Rgb([28, 28, 30]);
    let muted = Rgb([120, 120, 125]);

    // Title bar + horizontal rule
    fill_rect(&mut img, 72, 70, 420, 92, ink);
    fill_rect(&mut img, 72, 104, 648, 107, muted);

    // 4 paragraphs of word-like blocks
    let mut y = 135u32;
    for para in 0..4 {
        let lines = if para == 3 { 5 } else { 7 };
        for line in 0..lines {
            let mut x = 72u32;
            let end = if line == lines - 1 { 430 } else { 640 };
            let mut word = 0u32;
            while x + 28 < end {
                let wl = 22 + ((para * 13 + line * 17 + word * 29) % 44);
                let x1 = (x + wl).min(end);
                fill_rect(&mut img, x, y, x1, y + 8, ink);
                x = x1 + 10;
                word += 1;
            }
            y += 20;
        }
        y += 28;
    }
    img
}

/// Tall phone chat screenshot mock with status bar, header, alternating chat bubbles, input bar.
fn render_phone_chat() -> RgbImage {
    let (w, h) = (450, 900);
    let mut img = RgbImage::from_pixel(w, h, Rgb([236, 229, 221]));
    let header_bg = Rgb([18, 115, 98]);
    let status_bg = Rgb([12, 85, 72]);
    let white = Rgb([255, 255, 255]);
    let outgoing = Rgb([217, 253, 211]);
    let ink = Rgb([45, 45, 45]);

    // Status bar + app header
    fill_rect(&mut img, 0, 0, w, 34, status_bg);
    fill_rect(&mut img, 20, 11, 75, 23, white); // clock
    fill_rect(&mut img, w - 58, 11, w - 18, 23, white); // battery
    fill_rect(&mut img, 0, 34, w, 104, header_bg);
    fill_ellipse(&mut img, 50.0, 69.0, 22.0, 22.0, Rgb([210, 220, 225]));
    fill_rect(&mut img, 88, 56, 240, 72, white);
    fill_rect(&mut img, 88, 78, 170, 88, Rgb([180, 225, 215]));

    // Chat bubbles alternating left (white) and right (green)
    let bubbles: [(bool, u32, u32, u32); 7] = [
        (false, 130, 270, 68),
        (true, 215, 250, 62),
        (false, 295, 300, 88),
        (true, 400, 280, 72),
        (false, 490, 240, 62),
        (true, 570, 310, 88),
        (false, 678, 260, 68),
    ];
    for (is_right, y, bw, bh) in bubbles {
        let (x0, x1, bg) = if is_right {
            (w - 18 - bw, w - 18, outgoing)
        } else {
            (18, 18 + bw, white)
        };
        fill_rounded_rect(&mut img, x0, y, x1, y + bh, 14, bg);
        let rows = if bh > 75 { 3 } else { 2 };
        for r in 0..rows {
            let ly = y + 16 + r * 20;
            let lx1 = x1 - 24 - if r == rows - 1 { 45 } else { 0 };
            fill_rect(&mut img, x0 + 16, ly, lx1.max(x0 + 40), ly + 8, ink);
        }
    }

    // Bottom message input bar
    fill_rect(&mut img, 0, h - 76, w, h, Rgb([242, 242, 242]));
    fill_rounded_rect(&mut img, 16, h - 62, w - 76, h - 14, 22, white);
    fill_ellipse(
        &mut img,
        (w - 40) as f32,
        (h - 38) as f32,
        22.0,
        22.0,
        header_bg,
    );
    img
}

/// Smooth photo-like gradient landscape with blue sky, sun, layered mountains, and lake reflection.
fn render_gradient_landscape() -> RgbImage {
    let (w, h) = (960, 640);
    let mut img = RgbImage::new(w, h);
    let horizon = 410u32;

    for y in 0..h {
        for x in 0..w {
            let fx = x as f32 / w as f32;
            let rgb = if y < horizon {
                // Sky gradient: deep blue at top to warm pale cyan near horizon
                let t = y as f32 / horizon as f32;
                let r = (45.0 + 175.0 * t) as u8;
                let g = (115.0 + 120.0 * t) as u8;
                let b = (215.0 + 35.0 * t) as u8;

                // Distant mountain range 1 (cool blue-gray)
                let m1 = 270.0 - 85.0 * ((fx * 4.5 + 0.4).sin().abs()) - 35.0 * ((fx * 11.0).cos());
                // Closer mountain range 2 (darker green-slate)
                let m2 = 345.0 - 65.0 * ((fx * 6.0 + 1.7).sin().abs()) - 20.0 * ((fx * 15.0).sin());
                if y as f32 > m2 {
                    let mt = ((y as f32 - m2) / (horizon as f32 - m2)).clamp(0.0, 1.0);
                    Rgb([
                        (38.0 + 20.0 * mt) as u8,
                        (78.0 + 35.0 * mt) as u8,
                        (52.0 + 15.0 * mt) as u8,
                    ])
                } else if y as f32 > m1 {
                    let mt = ((y as f32 - m1) / (horizon as f32 - m1)).clamp(0.0, 1.0);
                    Rgb([
                        (78.0 + 30.0 * mt) as u8,
                        (96.0 + 30.0 * mt) as u8,
                        (128.0 + 25.0 * mt) as u8,
                    ])
                } else {
                    Rgb([r, g, b])
                }
            } else {
                // Lake water with subtle horizontal ripples & green shore banks
                let t = (y - horizon) as f32 / (h - horizon) as f32;
                let ripple =
                    (((y as f32 * 0.7).sin() * 6.0) + ((x as f32 * 0.05).cos() * 3.0)) as i32;
                let r = (40.0 + 25.0 * (1.0 - t)) as i32 + ripple;
                let g = (95.0 + 45.0 * (1.0 - t)) as i32 + ripple;
                let b = (155.0 + 55.0 * (1.0 - t)) as i32 + ripple;
                Rgb([
                    r.clamp(0, 255) as u8,
                    g.clamp(0, 255) as u8,
                    b.clamp(0, 255) as u8,
                ])
            };
            img.put_pixel(x, y, rgb);
        }
    }
    // Bright sun with soft halo
    fill_ellipse(&mut img, 760.0, 115.0, 60.0, 60.0, Rgb([250, 238, 175]));
    fill_ellipse(&mut img, 760.0, 115.0, 36.0, 36.0, Rgb([255, 250, 215]));
    img
}
