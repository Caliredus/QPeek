//! Sistem tepsisi ikonu: şeffaf zeminli canlı Swiss Q göstergesi.
//! Q halkası en yüksek kullanım yüzdesine göre saat yönünde dolar;
//! durum rengi <%50 yeşil, <%80 sarı, >=%80 kırmızıdır.

use std::f32::consts::{PI, TAU};

pub const SIZE: u32 = 32;

fn level_color(percent: Option<f32>) -> [u8; 3] {
    match percent {
        None => [156, 163, 175], // gri (bekleniyor)
        Some(p) if p >= 80.0 => [239, 68, 68],   // coral (kırmızı)
        Some(p) if p >= 50.0 => [245, 158, 11],  // amber (sarı)
        Some(_) => [16, 185, 129],               // emerald (yeşil)
    }
}

/// `percent` None ise veri yok demektir (boş/gri Swiss Q).
/// 4×4 alt-piksel yumuşatma (supersampling) ile 16–32 px tepsi boyutunda net kenarlar üretir.
pub fn gauge_rgba(percent: Option<f32>) -> Vec<u8> {
    let n = SIZE as usize;
    let mut px = vec![0u8; n * n * 4];

    let accent = level_color(percent);
    let fill_frac = percent.map(|p| p.clamp(0.0, 100.0) / 100.0).unwrap_or(0.0);
    // 0% iken bile üstte küçük bir başlangıç yayı görünsün; 100% tam daire olsun.
    let sweep = if percent.is_some() {
        (0.08 + fill_frac * 0.92).min(1.0) * TAU
    } else {
        0.0
    };

    let cx = 15.0_f32;
    let cy = 15.0_f32;
    let r_mid = 10.6_f32;
    let half_w = 2.45_f32;

    // Q kuyruğu: sağ alt çapraz çubuk (45° yönünde, halkayı kesip hafif dışa taşar).
    let tail_x0 = cx + 3.8;
    let tail_y0 = cy + 3.8;
    let tail_x1 = cx + 12.6;
    let tail_y1 = cy + 12.6;
    let tail_hw = 2.25_f32;

    // Uç gösterge noktası (dolu yayın bittiği uçta).
    let tip_angle = -PI / 2.0 + sweep;
    let tip_x = cx + r_mid * tip_angle.cos();
    let tip_y = cy + r_mid * tip_angle.sin();
    let dot_r = 2.15_f32;

    for y in 0..n {
        for x in 0..n {
            let mut acc = [0.0_f32; 4];
            for sy in 0..4 {
                for sx in 0..4 {
                    let fx = x as f32 + (sx as f32 + 0.5) * 0.25;
                    let fy = y as f32 + (sy as f32 + 0.5) * 0.25;
                    let sample = sample_swiss_q(
                        fx, fy, cx, cy, r_mid, half_w,
                        tail_x0, tail_y0, tail_x1, tail_y1, tail_hw,
                        sweep, percent.is_some(), tip_x, tip_y, dot_r, accent,
                    );
                    let a = sample[3];
                    acc[0] += sample[0] * a;
                    acc[1] += sample[1] * a;
                    acc[2] += sample[2] * a;
                    acc[3] += a;
                }
            }
            let a = acc[3] / 16.0;
            if a > 0.001 {
                let i = (y * n + x) * 4;
                px[i] = (acc[0] / acc[3]).clamp(0.0, 255.0).round() as u8;
                px[i + 1] = (acc[1] / acc[3]).clamp(0.0, 255.0).round() as u8;
                px[i + 2] = (acc[2] / acc[3]).clamp(0.0, 255.0).round() as u8;
                px[i + 3] = (a * 255.0).clamp(0.0, 255.0).round() as u8;
            }
        }
    }
    px
}

#[allow(clippy::too_many_arguments)]
fn sample_swiss_q(
    x: f32,
    y: f32,
    cx: f32,
    cy: f32,
    r_mid: f32,
    half_w: f32,
    tx0: f32,
    ty0: f32,
    tx1: f32,
    ty1: f32,
    tail_hw: f32,
    sweep: f32,
    has_data: bool,
    tip_x: f32,
    tip_y: f32,
    dot_r: f32,
    accent: [u8; 3],
) -> [f32; 4] {
    // 1) Q kuyruğu en üstte: beyaz çubuk + etrafında ince koyu ayrım boşluğu.
    let d_tail = dist_to_segment(x, y, tx0, ty0, tx1, ty1);
    if d_tail <= tail_hw {
        return [244.0, 246.0, 250.0, 1.0];
    }

    let dx = x - cx;
    let dy = y - cy;
    let r = (dx * dx + dy * dy).sqrt();

    // 2) Uç gösterge noktası (Swiss Q'nun imza renkli noktası).
    if has_data {
        let d_tip = ((x - tip_x).powi(2) + (y - tip_y).powi(2)).sqrt();
        if d_tip <= dot_r {
            return [accent[0] as f32, accent[1] as f32, accent[2] as f32, 1.0];
        }
        // Noktanın etrafında beyaz halka (yuvarlatılmış yay ucu).
        if d_tip <= half_w + 0.55 {
            return [244.0, 246.0, 250.0, 1.0];
        }
    }

    // 3) Q halkası.
    let d_ring = (r - r_mid).abs();
    if d_ring <= half_w {
        // Kuyruğun halkayla kesiştiği yerde ince ayrım boşluğu (Swiss tasarım detayı).
        if d_tail <= tail_hw + 1.05 {
            return [0.0, 0.0, 0.0, 0.0];
        }
        // Saat 12 yönünden (0) saat yönünde [0, TAU) açısı.
        let mut ang = dy.atan2(dx) + PI / 2.0;
        if ang < 0.0 {
            ang += TAU;
        }
        if has_data && ang <= sweep {
            // Dolu yay: durum renginde parlak yay (16×16'da bile anında okunur).
            return [accent[0] as f32, accent[1] as f32, accent[2] as f32, 1.0];
        }
        // Kalan (boş) kısım: kesikli ince gri halka.
        if d_ring <= half_w * 0.65 {
            let phase = (ang / TAU * 16.0).fract();
            if phase < 0.55 {
                return [185.0, 192.0, 204.0, 0.48];
            }
        }
    }

    // Başlangıç ucu yuvarlatması (saat 12).
    if has_data {
        let start_x = cx;
        let start_y = cy - r_mid;
        if ((x - start_x).powi(2) + (y - start_y).powi(2)).sqrt() <= half_w {
            return [accent[0] as f32, accent[1] as f32, accent[2] as f32, 1.0];
        }
    }

    [0.0, 0.0, 0.0, 0.0]
}

fn dist_to_segment(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> f32 {
    let vx = x1 - x0;
    let vy = y1 - y0;
    let len2 = vx * vx + vy * vy;
    if len2 <= 1e-6 {
        return ((px - x0).powi(2) + (py - y0).powi(2)).sqrt();
    }
    let t = (((px - x0) * vx + (py - y0) * vy) / len2).clamp(0.0, 1.0);
    let cx = x0 + t * vx;
    let cy = y0 + t * vy;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_valid_rgba_buffer() {
        for p in [None, Some(0.0), Some(35.0), Some(65.0), Some(92.0)] {
            let buf = gauge_rgba(p);
            assert_eq!(buf.len(), (SIZE * SIZE * 4) as usize);
            assert!(buf.chunks_exact(4).any(|px| px[3] > 200));
        }
    }
}
