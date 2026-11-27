//! Kota eşik (%80 / %100) ve sıfırlanma bildirimi takipçisi.
//! İlk yenilemede sessizce mevcut durumu kaydeder (açılışta bildirim yağmuru yapmaz),
//! yalnızca gerçek geçişlerde pencere ve hesap başına bir kez bildirim üretir.

use std::collections::HashSet;

use crate::backend::Card;
use crate::i18n::Lang;

type Key = (String, String, String); // (provider_id, account_key, metric_label_en)

#[derive(Default)]
pub struct AlertTracker {
    initialized: bool,
    armed_high: HashSet<Key>,
    armed_full: HashSet<Key>,
}

impl AlertTracker {
    /// Yeni çekilen kartları değerlendirir ve gönderilecek `(başlık, gövde)` bildirimlerini döndürür.
    pub fn observe(&mut self, cards: &[Card], lang: Lang, enabled: bool) -> Vec<(String, String)> {
        let mut toasts = Vec::new();
        let first = !self.initialized;
        self.initialized = true;

        for c in cards {
            if c.failed {
                continue;
            }
            let who = if c.email.is_empty() {
                c.name.clone()
            } else {
                format!("{} ({})", c.name, c.email)
            };

            for m in c.bars() {
                let key: Key = (c.id.clone(), c.account_key.clone(), m.label_en.clone());
                let win_label = m.label(lang);

                if m.percent >= 99.5 {
                    self.armed_high.insert(key.clone());
                    if self.armed_full.insert(key) && !first && enabled {
                        let (title, body) = match lang {
                            Lang::Tr => (
                                format!("{} — Limit doldu", c.name),
                                format!("{who} · {win_label} kotası %100'e ulaştı."),
                            ),
                            Lang::En => (
                                format!("{} — Limit reached", c.name),
                                format!("{who} · {win_label} quota reached 100%."),
                            ),
                        };
                        toasts.push((title, body));
                    }
                } else if m.percent >= 80.0 {
                    // %100'den düştüyse sıfırlanmış/boşalmış say.
                    if self.armed_full.remove(&key) && m.percent < 90.0 && !first && enabled {
                        toasts.push(restored_toast(&c.name, &who, win_label, lang));
                    }
                    if self.armed_high.insert(key) && !first && enabled {
                        let pct = m.percent.round() as i32;
                        let (title, body) = match lang {
                            Lang::Tr => (
                                format!("{} — Yüksek kullanım (%{pct})", c.name),
                                format!("{who} · {win_label} kotası %{pct} oldu."),
                            ),
                            Lang::En => (
                                format!("{} — High usage ({pct}%)", c.name),
                                format!("{who} · {win_label} quota reached {pct}%."),
                            ),
                        };
                        toasts.push((title, body));
                    }
                } else {
                    if self.armed_full.remove(&key) && !first && enabled {
                        toasts.push(restored_toast(&c.name, &who, win_label, lang));
                    }
                    if m.percent < 75.0 {
                        self.armed_high.remove(&key);
                    }
                }
            }
        }
        toasts
    }
}

fn restored_toast(name: &str, who: &str, win_label: &str, lang: Lang) -> (String, String) {
    match lang {
        Lang::Tr => (
            format!("{name} — Kota sıfırlandı"),
            format!("{who} · {win_label} tekrar kullanıma hazır."),
        ),
        Lang::En => (
            format!("{name} — Quota reset"),
            format!("{who} · {win_label} is ready to use again."),
        ),
    }
}

/// Ayarlar'daki "Test" düğmesinin gönderdiği örnek bildirim.
pub fn test_toast(lang: Lang) -> (String, String) {
    match lang {
        Lang::Tr => (
            "QPeek · Test Bildirimi".into(),
            "Claude (5 saat) kotası sıfırlandı — tekrar kullanıma hazır.".into(),
        ),
        Lang::En => (
            "QPeek · Test Alert".into(),
            "Claude (5h) quota reset — ready to use again.".into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Metric;

    fn card(pct: f32) -> Card {
        Card {
            id: "claude".into(),
            name: "Claude".into(),
            email: "a@b.c".into(),
            account_key: "default".into(),
            plan: String::new(),
            metrics: vec![Metric {
                label_tr: "5 saat".into(),
                label_en: "5h".into(),
                percent: pct,
                detail_tr: String::new(),
                detail_en: String::new(),
                pace_tr: String::new(),
                pace_en: String::new(),
                pace_level: 0,
                info: false,
                window_minutes: Some(300),
                resets_at: None,
            }],
            failed: false,
        }
    }

    #[test]
    fn alerts_fire_once_per_threshold_and_on_reset() {
        let mut t = AlertTracker::default();
        // İlk yoklama açılışta sessizdir.
        assert!(t.observe(&[card(40.0)], Lang::Tr, true).is_empty());
        // %82 -> tek uyarı.
        assert_eq!(t.observe(&[card(82.0)], Lang::Tr, true).len(), 1);
        assert!(t.observe(&[card(85.0)], Lang::Tr, true).is_empty());
        // %100 -> doldu uyarısı.
        assert_eq!(t.observe(&[card(100.0)], Lang::Tr, true).len(), 1);
        assert!(t.observe(&[card(100.0)], Lang::Tr, true).is_empty());
        // %5 -> sıfırlandı uyarısı.
        let r = t.observe(&[card(5.0)], Lang::Tr, true);
        assert_eq!(r.len(), 1);
        assert!(r[0].0.contains("sıfırlandı"));
    }
}
