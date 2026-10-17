# 0.2–0.5 yerel kabul kaydı — 2026-10-17

Bu belge canlı model başarımı raporu değildir.

| Aşama | Kaynak kabulü | GitHub commit |
| --- | --- | --- |
| 0.2 | Shell/arama/patch/diff ve araç politikası; 45 test | ee41396 |
| 0.3 | Özel atomik session/config, etki günlüğü ve resume; 55 test | 550c86f |
| 0.4 | Native device OAuth, refresh, Responses/replay; 68 test | f8c0b2b |
| 0.5 | Rust benchmark, bağımsız orakllar, RSS, Jcode köprüsü; 78 test | Bu belgeyle birlikte yayımlanır |

Son testler: 78 geçti, 0 başarısız, 0 atlandı. Workspace Clippy uyarılar hata
olacak şekilde geçti; format kontrolü ve optimize release build geçti.

Release `bench demo`: fixture modu, 300 saniye bütçe, 3 görev × 2 ajan:

| Fixture | Oracle sonucu |
| --- | --- |
| Known-solution native SDK/araç yazıcısı | 3/3 |
| Yalnızca “done” yazan no-op | 0/3 |

Her görev ayrı çalışma klasöründe, ajanlar sırayla koştu. Prompt/girdi hash'leri
raporlandı. Çıkış 0 tek başına başarı sayılmadı. Tam eşleşme/semantik checker'lar
sonucu belirledi. Mode açıkça fixture; Jcode/model üstünlüğü iddiası yoktur.

Resmî Jcode v0.98.1 (1c7426eb1) release'i bu cloud workspace'e kuruldu. İndirilen
x86_64 arşivin SHA-256'sı GitHub release digest'iyle eşleşti; `--version`/`run
--help` çalıştı. Binary ve özel auth/session/log dosyaları Git'e eklenmedi.

Canlı Codex device login başlatıldı; kullanıcı onayı bekleniyor. Hesap/model
erişimi ve gerçek Namzu/Jcode karşılaştırması henüz yapılmadı. Yeniden giriş
gerekirse `auth login` açıkça başlatılır; kayıtlar/tokenlar bu belgede tutulmaz.
