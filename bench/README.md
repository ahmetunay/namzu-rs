# 0.5 — deterministik benchmark

Bu araç kayıp eski test_bench.py/bench.py dosyalarının kurtarılmış kopyası değildir.
Yeni, sürümlü JSON sözleşmesi olan native Rust benchmark koşucusudur.

## Çalışma biçimi

- Ajanları sırayla çalıştırır; görev başına varsayılan ve en fazla **300 saniye**.
- Aynı model, prompt ve başlangıç dosyaları kullanılır. Her ajan/görev/tekrar için
  ayrı temiz çalışma klasörü oluşturulur; önceki ajanın dosyası diğerine geçmez.
- Tekrarlarda ajan sırası tersine çevrilir. Paralel koşum yoktur.
- Başarı bağımsız expected_files ve/veya test komutu ile ölçülür. “done” yazısı,
  exit 0 veya ajanın kendi test sonucu tek başına başarı sayılmaz.
- Agent error, timeout, output_limit ve incorrect ayrı raporlanır. Hata exit'i,
  dosya doğru olsa bile ajan başarısı sayılmaz.
- Prompt/girdi/executable SHA-256, elapsed_ms, exit code ve uygun olduğunda
  provider/tool çağrı sayıları kaydedilir. Model token/ücret alanları henüz yok;
  ücret veya kullanılmayan çağrı sayısı için sıfır uydurulmaz.
- `wait4.ru_maxrss` KiB ölçülür. Bu **eşzamanlı tüm süreçlerin toplam RAM/PSS'si
  değildir**; süreç ve beklenmiş çocuklardaki en büyük bireysel yüksek su izidir.
  Daemon, paylaşılan sayfalar ve grup toplamı için sonraki eval aşaması gerekir.
- Çalışma klasörü süreç sandbox'ı değildir. Yalnızca güvenilen ajan/test kodunu
  çalıştır. Adversarial ajanın host'taki oracle kodunu değiştiremeyeceği iddiası yok.

## Kota harcamayan smoke testi

```sh
cargo build --release --workspace --locked
./target/release/namzu-rs bench demo --out artifacts/bench
```

Üç görev: Unicode slug, aralık birleştirme, deterministik topolojik sıralama.
Known-solution fixture aynı SDK/araçları kullanarak solution.py yazar; no-op
fixture yalnızca “done” der. Beklenen sonuç 3/3 ve 0/3'tür. Bu sonuç bir modelin
veya Jcode'un başarımı olarak yorumlanmaz. Rapor `mode: fixture` taşır.

## Canlı Namzu/Jcode karşılaştırması

Önce iki ajan aynı aboneliğe bağlanmış ve aynı model iki tarafta bulunmuş olmalı.
Login canlı, normal fixture testleri offline'dır. Ortak modeli tahmin etmek yerine
Codex'in gerçek kataloğundan seç:

```sh
./target/release/namzu-rs auth login
./target/release/namzu-rs models
./target/release/namzu-rs auth link-jcode --directory /ozel/yeni-jcode-profili
export JCODE_HOME=/ozel/yeni-jcode-profili
./target/release/namzu-rs bench init --model ORTAK_MODEL_ID \
  --jcode /kurulu/jcode --file artifacts/live-suite.json
./target/release/namzu-rs bench validate artifacts/live-suite.json
./target/release/namzu-rs bench run artifacts/live-suite.json --out artifacts/bench
```

`auth link-jcode`, açıkça seçilen YENİ özel profile erişim ve identity tokenlarını
kopyalar; mevcut openai-auth.json dosyasının üstüne yazmaz. Refresh grant Namzu'da
kalır; iki uygulamanın aynı grant'i döndürerek birbirini bozması engellenir.
Jcode abonelik modunu identity tokenının varlığıyla tanır; refresh grant kopyalanmaz.
Eski Namzu kayıtlarında identity tokenı yoksa köprü, kendi auth kilidi altında
bir kez yenileyerek servisten alır ve atomik saklar.
Bu tek seferlik kopya token süresi dolunca yenilenmez; uzun kullanım için Jcode'a
ayrı login yap veya yeni profile yeni kopya oluştur. Tokenlar stdout'a, suite'e
veya GitHub'a yazılmaz. Jcode'un özel secret-store şeması kullanılır.

`init` üç built-in görevi ve özel çalışma klasörlerini kullanacak argv'leri yazar;
model çağrısı yapmaz. Oluşturduğu araç izin profilini incele. Namzu'ya write/shell,
Jcode'a bash/read/write/edit/apply_patch/glob/grep verilir; Jcode güncellemesi ve
selfdev kapatılır, ayrı socket kullanılır. Her iki ajan host dosya/shell yetkilidir.
`permission_profile` ortak bir **beyan**dır; farklı ürünlerin araç davranışlarını
ve iç adım/bağlam limitlerini koşucu bağımsız olarak aynı hale getirmez.
Bu sınırlar rapor yorumunda belirtilmelidir. Benchmark sonucu dilin tek başına
RAM/başarım etkisini kanıtlamaz; model ve ajan politikaları da etkilidir.

Jcode bu workspace'te resmî v0.98.1 release asset'inden kurulmuştur. GitHub'ın
bildirdiği arşiv SHA-256 doğrulandı:
`449a8ed50aa9ac75e6afe9b8e916814172ff7cc63913c7ae1f9abd5b1bc5323c`.
İkili kaynak deposuna eklenmez. Başka bir makinede Jcode'u ayrıca kurmalısın.

Canlı giriş/model doğrulaması tamamlanmadan fixture sonuçlarını canlı
karşılaştırma diye yayımlama. Cihaz onayı alındı ve native model kataloğuna erişim doğrulandı;
canlı koşum sonucu ayrı kabul kaydında tutulur.

## JSON sözleşmesi

Suite: `version`, `mode` (fixture/live), `model`, `timeout_seconds`, `repeats`,
`agents`, `tasks`. Bilinmeyen alanlar reddedilir. Örnek: live.example.json.

Agent: `name`, `program`, `args`, `permission_profile`, `inherit_env` (yalnızca
aktarılacak değişken ADLARI). Anahtarlar config'e yazılmaz, env değerleri raporlanmaz.
Program argv ile doğrudan başlatılır; prompt shell'e birleştirilmez. Tek geçişli
yer tutucular: `{prompt}`, `{workspace}`, `{state_dir}`, `{model}`, `{task}`,
`{run_id}`. Prompt içindeki yer tutucular yeniden yorumlanmaz.

Task: `id`, `prompt`, `files`, `expected_files`, `check`. En az bir bağımsız oracle
gerekir. `check` argv'sinde `{suite_dir}` ve `{workspace}` desteklenir. Check
programı modelden bağımsız host seçimidir; kendi 30 saniye/32 KiB bütçesi vardır.
Başlangıç dosyaları relative olmalı; mutlak/parent traversal reddedilir.

Report: `version`, `mode`, `model`, `timeout_seconds`, `sequential`,
`suite_sha256`, `memory_scope`, `runs`, `summary`, `output_directory`.
Her koşunun status/oracle_passed/exit_code/elapsed_ms/peak_rss_kib ve hash'leri var.
`stats` yalnızca tanınan sayısal provider_calls/tool_calls alanlarını alır;
uyumsuz ajan çıktılarında null olabilir. Seçilen executable bir launcher ise
program_sha256 yalnızca o launcher'ı tanımlar, tüm bağımlı dosyaları tanımlamaz.

Koşucu bitirip rapor ürettiyse CLI exit 0 döner; görevlerin başarısını runs/status
ve summary üzerinden değerlendir. Suite/runtime hatası exit 1'dir. Run progress
stderr, sonuç JSON stdout'tur. Rapor her tamamlanan koşudan sonra atomik kaydedilir.
İptalde yeni koşu başlamaz; önceki tamamlanan satırlar korunur.

Rapor ve ham stdout/stderr özel 0700/0600 artifacts dizininde kalır. Ham çıktı
özel görev içeriği içerebilir; GitHub'a otomatik yüklenmez. Yayımlanacak kabul
kaydı yalnızca gözden geçirilmiş sayısal/anonim özet olabilir.
