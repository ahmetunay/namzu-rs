# 0.2–0.5 yerel ve canlı kabul kaydı — 2026-10-17

Yerel fixture kabulü ve ilk canlı pilot ayrı kaydedilmiştir.

| Aşama | Kaynak kabulü | GitHub commit |
| --- | --- | --- |
| 0.2 | Shell/arama/patch/diff ve araç politikası; 45 test | ee41396 |
| 0.3 | Özel atomik session/config, etki günlüğü ve resume; 55 test | 550c86f |
| 0.4 | Native device OAuth, refresh, Responses/replay; 68 test | f8c0b2b |
| 0.5 | Rust benchmark, bağımsız orakllar, RSS, Jcode köprüsü; 78 test | 7f42c53 |

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

## Canlı kabul

Cihaz girişi kullanıcı tarafından onaylandı. Native Namzu sürücüsü gerçek model
kataloğunu okudu; Jcode da aynı hesapta GPT-6-Luna modelini gördü. Köprü, Jcode'un
abonelik modunu tanıması için erişim ve identity tokenlarını aktarır; refresh grant
Namzu'da kalır. Eski kayıt için bir gerçek refresh yapıldı ve dönen tokenlar
atomik saklandı. Hesap bilgisi/tokenlar bu depoya eklenmedi.

Canlı kabul sırasında iki entegrasyon düzeltildi:

- `da62fff`: Jcode abonelik/API-key ayrımı için identity tokenını koru/aktar;
  eski kaydı auth kilidi altında bir kez yenile. Regresyon testiyle toplam 79 test.
- `c4dcbac`: Content-Type'a bağlı kalmadan sınırlı gerçek SSE içeriğini doğrula;
  completed yanıt gelmeden araç çalıştırma. Native gerçek model smoke testi geçti.

Ardından ilk tam pilotta iki ajan da 2/3 aldı: slug/toposort geçti, intervals
başarısızdı. Görev metni Python bool değerleri konusunda yeterince açık değildi;
bağımsız oracle zaten exact int bekliyordu. `6d6a40a` yalnızca görev metnini
bool/float reddini açık yazacak şekilde netleştirdi; oracle veya çözüm değiştirilmedi.
İki ajan da yeni, eşit girdilerle üç görevin tümünde tekrar çalıştırıldı. Bu yüzden
iki koşuyu aynı görev sürümüymüş gibi birleştirmiyoruz. Başlangıçtaki SSE entegrasyon
hatası olan kesilmiş koşu da bu sonuçlara katılmadı.

Son kabul: GPT-6-Luna (`gpt-6-luna`), iki tarafta low effort, HTTPS, 1 tekrar,
seri yürütme, her ajan/görev için 300 saniye ve ayrı başlangıç klasörleri.
Jcode v0.98.1; Namzu kaynak commit'i `6d6a40a`. Kaynak kod ve binary/hash kimlikleri
[redakte JSON raporunda](live-acceptance-2026-10-17.json).

| Görev | Namzu Rust | Jcode |
| --- | --- | --- |
| slug | Geçti, 5.187 s | Geçti, 9.377 s |
| intervals | Geçti, 5.813 s | Geçti, 15.716 s |
| toposort | Geçti, 8.694 s | Geçti, 15.270 s |

| Ölçüm | Namzu Rust | Jcode |
| --- | --- | --- |
| Başarı | 3/3 | 3/3 |
| Toplam ajan süresi | 19.694 s | 40.363 s |
| Üç koşudaki en büyük peak RSS | 11.38 MiB | 45.40 MiB |

Bu küçük pilotta Namzu daha kısa sürede, daha düşük ölçülen peak RSS ile tamamladı.
Tek tekrar istatistiksel üstünlük veya dil etkisi kanıtlamaz. İkisi de Rust'tır;
Namzu'nun bu sürümü daha küçük bir özellik kümesidir. Sistem prompt'ları, araçların
uygulamaları ve iç bağlam/adım sınırları farklıdır. Ajan sırası bu tekrarda hep
Namzu → Jcode; servis gecikmesi, önbellek ve sıra etkileri kontrol edilmedi.
Peak RSS eşzamanlı bütün süreçlerin toplam RAM/PSS'si değildir. Oracle süreleri
ajan sürelerine dâhil değildir; Jcode provider/tool çağrı sayıları null bırakıldı.
Token/ücret veya gelecekteki kota garantisi çıkarılmaz.

Son kaynak kontrolleri: **79 test geçti**, format, Clippy (`-D warnings`) ve
optimize release build geçti. Normal testler model kotası tüketmedi. Canlı araç
çağrılı görevler, opaque reasoning replay ve tamamlanan session'ın yeni CLI
çağrısında devamı ayrıca doğrulandı. TUI/Pals/MCP bu kabulün kapsamında değildir.
