# 7.3 — deterministik değerlendirme

Mevcut `namzu-bench` SDK ve seri süreç koşucusu genişletildi. Başarı; ajanın
son mesajından değil, host'un seçtiği bağımsız dosya/test orakllarından gelir.
Bilinen fixture çözümleri ölçüm altyapısını sınar; model kalitesi ölçmez.

## Kullanım ve görevler

```sh
cargo build --release --workspace --locked
./target/release/namzu-rs bench demo --task-set eval --out artifacts/evals
python3 scripts/eval_smoke.py
```

`--task-set algorithms` varsayılandır; eski üç görevlik demo/init değişmez.
`--task-set eval` beş görevi seçer:

| Görev | Beklenen davranış ve bağımsız orakl |
| --- | --- |
| `slug` | Unicode NFKD/ASCII slug, boş girdi ve ayraçlar |
| `intervals` | Dokunan aralıklar, exact int doğrulama, bool reddi, girdi değişmezliği |
| `toposort` | Leksikografik seçim, döngü/bilinmeyen düğüm reddi, sabit tohumlu DAG'lar |
| `ledger` | Başlangıçtaki hatalı kodu düzeltme; ilk ID kazanır, negatif/sıfır toplamlar, sıralı anahtarlar, her satırda tip doğrulama |
| `refactor` | Kopya normalizasyonu `legacy.normalize` çağrısına taşıma; Unicode/exception/generator davranışı ve legacy.py baytlarının korunması |

Ledger oraklı sabit `7303` tohumuyla 80 ek veri kümesi üretir. Referans hesabı
aday çözümden bağımsızdır. Refactor oraklı legacy fonksiyonunu probe ile değiştirip
gerçek delegasyon ve çağrı sırasını sınar. 7.4 düzeltmesiyle probe sonrası aday
yeniden yüklenir; doğrudan fonksiyon importu da geçerli delegasyon sayılır. Sadece aynı metni döndüren kopya kod
geçmez. Legacy dosyası ayrıca exact-byte gate ile denetlenir. Testler workspace'e
başlangıç dosyası olarak verilmez; Python `-I -c` ile host tarafından çalıştırılır.
Bu, kötü niyetli host yetkili bir ajana karşı gizlilik veya izolasyon garantisi değildir.

İlerideki canlı karşılaştırma için yalnızca suite hazırlama, model çağırmaz:

```sh
./target/release/namzu-rs bench init --task-set eval --model ORTAK_MODEL \
  --jcode /kurulu/jcode --file artifacts/eval-suite.json
./target/release/namzu-rs bench validate artifacts/eval-suite.json
```

7.3 kabulünde canlı çağrı yapılmadı. Karşılaştırma koşulları ve gerçek model
koşuları 7.4'ün kapsamıdır. Eski üç görevlik canlı pilot bu yeni kabulün sonucu değildir.

## Dosya ve test kapıları

Suite JSON sürümü hâlâ **1**. Task alanlarına isteğe bağlı iki bütçe eklendi:
`check_timeout_seconds` varsayılan/en fazla 30 (en az 1), `check_output_limit`
varsayılan/en fazla 32768 bayt (en az 1). Ajan süresi ayrı olarak varsayılan/en
fazla **300 saniye**. Check bütçesi ajan süresine dahil değildir.

`expected_files` ve `check` birlikte verildiyse ikisi de geçmelidir; bir dosya
yanlış olduğunda da test çalışır. Boş orakl reddedilir. Dosyalar byte-for-byte
karşılaştırılır; newline/encoding farkı da farktır. İçerik limiti 64 KiB'dir.
SDK giriş noktası `namzu_bench::oracle::evaluate`, typed `OracleResult`,
`FileOutcome`, `CheckOutcome` ve `RunStatus` sunar. Ayrı dönen checker stdout/stderr
değeri özel log içindir; normal değerlendirme raporuna eklenmez.

| Dosya outcome | Anlamı |
| --- | --- |
| `passed` | Beklenen baytlarla tam eşleşme |
| `missing` | Dosya bulunamadı |
| `unsafe_path` | Normal dosya değil, son bileşen symlink veya metadata erişimi reddedildi |
| `too_large` | Dosya 64 KiB sınırını aştı |
| `mismatch` | Baytlar farklı |
| `read_error` | Normal dosya okuması tamamlanamadı; ölçüm yok |

| Check outcome | Anlamı |
| --- | --- |
| `passed` | Normal çıkış, code 0 |
| `test_failed` | Normal çıkış, sıfır olmayan code; syntax/import/assertion gibi nedenler özel logda |
| `timeout` / `output_limit` | Checker bütçesi aşıldı; ölçüm yok |
| `error` | Checker başlatma/sinyal/gözetim hatası; ölçüm yok |

Checker herhangi bir sıfır olmayan normal çıkışı test başarısızlığı sayar; özel
bir checker bozuk yapılandırma için de aynı çıkışı kullanırsa koşucu bunu ayırt
edemez. Yerleşik görevler bu çıkış sözleşmesine göre sınanır.

## Sonuçlar ve rapor sürümü 2

Rapor **version: 2** oldu; tüketiciler yeni sürümü açıkça ele almalı. Eski temel
alanlar korundu; `oracle_passed` artık `true` / `false` / `null` olabilir.
Zorunlu bir ölçüm alınamadıysa `null` olur; varsa diğer gate sonuçları korunur.

| Koşu status | Anlamı |
| --- | --- |
| `passed` | Ajan exit 0 ve bütün orakllar geçti |
| `incorrect` | Ajan exit 0, ölçümler tamamlandı, en az bir gate başarısız |
| `agent_error` | Ajan sıfır olmayan çıkış/sinyal/başlatma/gözetim hatası |
| `timeout` / `output_limit` | Ajanın süre/çıktı sınırı aşıldı |
| `oracle_error` | Ajan exit 0, zorunlu orakl ölçümü alınamadı |

Ajan yürütme hatası koşu status'unda önceliklidir; dosya doğru olsa da başarı
sayılmaz. Ajan ve orakl sonuçları ayrı tutulur. Summary her ajan için `passed`,
`incorrect`, `execution_errors`, `oracle_errors`, `total` verir; bu dört hata/
başarı kategorisinin toplamı `total` olur. Ölçüm hatası doğruluk hatasına eklenmez.

Her satırda `execution_status`, ayrıntılı `oracle.files`/`oracle.check`, ayrı
`oracle.elapsed_ms` ve `oracle_sha256` bulunur. Orakl hash'i beklenen dosya
içerikleri, checker argv'si ve bütçelerini kapsar. Yerleşik checker kaynakları
argv içinde gömülüdür; özel dış script dosyalarının içeriğini bu hash kapsamaz.
Suite hash'i prompt/girdi/orakl yapılandırmasını; program hash'i seçilen ajan
executable'ını kapsar. Bağımlılıklar veya ortamın tamamı bu hash'lere dahil değildir.
Tekrar üretilebilir olan fixture sonuçları ve tanım hash'leridir; süre/RSS veya
canlı model yanıtlarının birebir aynı olması beklenmez.

Üst `status: running`, `planned_runs` ve tamamlanan satırlar her koşudan sonra
atomik kaydedilir. Yalnızca koşucu sona erdiğinde `status: complete` ve summary
yazılır. İptal/çökmede son tamamlanmış satırlar kalır; `running` rapor bitmiş deney
sayılmaz. CLI exit 0, raporun tamamlandığını gösterir; tüm görevlerin geçtiği
anlamına gelmez. Geçersiz suite/çözümlenemeyen komutlar bütün ajan etkilerinden
önce reddedilir. Çalışma sırasındaki süreç hataları satıra kaydedilip sonraki
koşuya devam edilir; iptal ve kayıt/host dosya sistemi hataları deneyi durdurur.

## Kabul ve sınırlar

7.3 kabulünde 13 yeni Rust davranış testiyle toplam **651 test** geçti. Format, Clippy
(`-D warnings`), release workspace build ve **9 release CLI kabul grubu** geçti.
Kabul komutu tekrar çalıştırılabilir; `artifacts/eval-acceptance/report.json`
başlangıçta `running` yazılır, hata veya kesintide eski başarılı rapor gösterilmez.

Kapsam: beş doğru fixture 5/5; boş/hatalı başlangıç çözümleri 0/5; iki demo
koşusunda aynı kararlar ve hash'ler; dosya/ajan hatası ayrımı; özel checker logu;
ölçülemeyen checker; etki öncesi geçersiz bütçe/komut reddi; inference yapmadan
genişletilmiş init/validate. Rust testleri ayrıca orakl timeout/crash/launch,
ajan timeout/çıktı taşması/crash/launch, bütün gate'lerin çalışması, symlink/büyük
dosya reddi, protected-file değişikliği ve iptalde sonraki workspace'in oluşmamasını sınar.

Bu kapsam upstream `packages/sdk/src/eval/{types,scorers,experiment}.ts`
altyapısının tamamı değildir. Referans commit:
`a8a751ab157ba5392db107ee6c589575336b1711`. Eksik ölçümün yanlış çözümle
karıştırılmaması ilkesi korundu; LLM judge, fuzzy scorer/trajectory puanı,
belirsizlik aralıkları ve upstream deney API'si taşınmadı.

Orakllar ve ajanlar güvenilen host süreçleridir; workspace OS sandbox değildir.
Sabit fixture çıktıları model başarısı değildir. Provider/tool sayaçları ajanın
raporladığı sayılardır, bağımsız denetlenmez; bilinmeyenler null kalır. Token/ücret
bu benchmark sözleşmesine eklenmedi. RSS tüm süreçlerin eşzamanlı toplam RAM'i
değildir. Dosya yolları ve görev tanımları içerik taşıyabilir: eval raporu özel
artifact'tir, içeriksiz telemetry kaydıyla aynı gizlilik sözleşmesine sahip değildir.
Ham ajan/checker çıktıları 0600 özel loglarda kalır; kabul raporları model kotası
harcamaz ve üretilen artifact'ler GitHub'a yüklenmez.

7.4 canlı deneyinde oraklın doğrudan fonksiyon importunu yanlış reddettiği
saptandı. Bir yeni regresyon testiyle toplam 652 Rust testi; düzeltilmiş oraklla
20 kayıtlı çıktı sıfır yeni inference ile yeniden sınandı. İlk kararlar silinmedi.
[Yanlış ret, düzeltme ve canlı sonuçlar](COMPARISON.md).
