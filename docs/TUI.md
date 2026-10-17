# Terminal mesaj ekranı — 1.1–1.3

Native Rust `namzu-tui`, Ratatui/Crossterm ile gerçek terminalde çalışır. Çekirdek
TUI'ye bağımlı değildir. `exec --json` ve benchmark komutları kullanılmaya devam eder.

```sh
cargo build --release --workspace --locked
./target/release/namzu-rs tui --demo
./target/release/namzu-rs tui --provider codex --model gpt-6-luna --effort low
```

Demo tamamen scripted bir SDK/araç akışıdır: seçilen proje yerine geçici klasöre
answer.txt yazar/okur, 12 yanıtını gösterir, model/kimlik bilgisi kullanmaz ve
konuşma kaydetmez. Girilen mesaj demo çözümünü değiştirmez. Gerçek model ID'sini
`namzu-rs models` ile kendi hesabının kataloğundan seç. TUI otomatik login açmaz.

Varsayılan araçlar salt okunurdur. `--allow-write`, `--allow-shell`,
`--tool-allow`, `--tool-deny` exec ile aynı şekilde uygulanır. Shell, host dosya
ve ağına erişim iznidir; OS sandbox'ı değildir. TUI yeni yetki vermez. İzin tavanı içindeki write_file/apply_patch/shell çağrıları
her defasında y/n ile tek seferlik onay ister. n, modele araç hatası olarak döner;
Ctrl-C turn'ü iptal eder, Esc uygulamadan çıkar. Onaylar oturuma kaydedilmez.
Salt okuma araçları otomatik geçer; tavan dışında kalan araç onayla açılamaz.
64 KiB'den büyük onay argümanları gösterilmeden reddedilir. `--config`, `--state-dir`, `--cwd`,
`--timeout`, adım/araç bütçeleri ve `--resume` ortak çalıştırma yolunu kullanır.

## Tuşlar ve konuşma

| Tuş | Davranış |
| --- | --- |
| Enter | Mesajı gönder |
| F2 / Alt-Enter | Yeni satır ekle |
| Oklar / Home / End / Delete | Unicode imleç hareketi ve düzenleme |
| F3 / F4, Tab | Önceki/sonraki araç kartını seç, ayrıntıyı aç/kapat |
| Backspace | Son Unicode grapheme'ini sil |
| PageUp / PageDown | Mesaj geçmişini kaydır |
| Ctrl-C | Çalışan turn'ü iptal et; boşta ise çık |
| Ctrl-N | Boştayken açıkça yeni konuşma başlat |
| Esc | Çık, çalışan turn'ü iptal et ve worker'ı birleştir |

Yapıştırılan satır sonları korunur; CRLF normalleştirilir. Editör en fazla beş
görünür satırla imleci izler. Araç kartı çalışırken durum ve geçen süreyi gösterir;
sonuç ilk üç satırla özetlenir, Tab ile 64 KiB sınırındaki ayrıntı açılır.
apply_patch/diff_file sonuçlarında eklenen/silinen/hunk satırları renklendirilir.

Başarılı turn'ler aynı kalıcı session ile devam eder. `--resume ID` tamamlanan
bir session'ın bağlamını kullanabilir; ilk ekranda eski mesajlar yeniden yüklenmez,
yalnızca yeni turn'ler gösterilir. Session seçicisi ve geçmiş yükleme 1.4'tedir.
Hatalı/iptal edilmiş turn'den sonra Enter kapalıdır: Ctrl-N ile yeni konuşma
başlatılır. Belirsiz dosya/shell etkisi otomatik tekrarlanmaz. Çökme/hata sonrası
session durumu, exec'teki özel kayıt ve muhafazakâr resume kurallarına tabidir.

## Olay ve güvenilirlik sözleşmesi

- `Provider::complete_stream` isteğe bağlıdır; eski sağlayıcılar `complete`
  üzerinden tam yanıtı iletmeye devam eder. OpenAI-compatible API adaptörü
  tamponlu Chat Completions kullanır. Codex sürücüsü native SSE metin delta'larını iletir.
- `Event` / `EventSink` ve `Observer::event` mesaj/araç/turn durumunu taşır.
  `AssistantDelta` geçici görünüm içindir; `AssistantMessage` doğrulanmış yanıtı
  kabul eder ve taslağı değiştirir. Kesik/failed/incomplete SSE taslağı silinir.
- Reasoning, opaque replay, tokenlar ve tool-argument delta'ları görüntü olayına
  aktarılmaz. Olaylar görev içeriği taşıyabilir; genel telemetri/log akışı değildir.
- Araç çalıştırmak için tamamlanmış sağlayıcı yanıtı, tüm batch'in ID/argüman/bütçe
  doğrulaması, kalıcı pending checkpoint ve iptal kontrolü hâlâ zorunludur.
  UI olayları kalıcı journal'ın yerini almaz. UI tüketicisi koparsa turn durur.
- Tek worker, en fazla 64 bekleyen olay ve geri basınç vardır. Görünüm geçmişi
  256 KiB/256 entry, tek mesaj/taslak 64 KiB, giriş 16 KiB ile sınırlıdır. Tam
  kabul edilen model konuşması önceki SDK/session bütçelerine tabidir.
- Model, araç, dosya ve kullanıcı metinlerinin terminal kontrol karakterleri
  etkisiz gösterilir. Unicode satır sarma ve giriş silme grapheme farkındadır.
- stdin ve stdout terminal olmalıdır. Pipe'ta TUI başlamadan açık hata verir;
  programatik kullanım için `exec --json` kullanılır. Normal çıkış, turn hatası
  ve Ctrl-C/Esc akışlarında raw mode, cursor, alternate screen ve bracketed paste
  geri yüklenir. Worker bırakılmaz; süreç grubu temizliği mevcut supervisor'dadır.

HTTP istek/başlık/gövde/SSE beklemeleri async transport üzerinden iptal ve deadline
ile yarışır; iptalde future düşürülür, kopuk ağ worker'ı bırakılmaz. Proxy/CA ve
yönlendirme politikası korunur. Onay bekleme süresi turn bütçesine dâhildir.
OS dosya I/O'su, DNS'in engellenen sistem çağrıları ve SIGKILL için anlık kesilme
garantisi yoktur. Session seçimi ve kalan terminal yaşam döngüsü 1.4'tedir.

## Doğrulama

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
python3 scripts/tui_smoke.py
```

Linux PTY kabul script'i Python 3.11+ kullanır: gerçek terminalde offline demo,
yerel HTTP araç→yanıt→session resume, hata→açık yeni konuşma, shell sırasında
Ctrl-C ve Esc. Terminal modları, alternate screen, child pidfd çıkışı ve iptal
sonrası ikinci aracın çalışmaması kontrol edilir. Script normalde model çağırmaz.
Canlı hesabı açıkça kullanmak için `--live-model GERCEK_MODEL_ID` eklenebilir;
bu seçeneğin ayrı bir read-only görevi hesap kotası tüketir.

[1.1 kabul kaydı](TUI-ACCEPTANCE.md). TUI modülünün bütünü henüz tamamlanmadı:
1.1–1.3 tamamlandı; 1.4 oturum/model/bütçe ekranları sıradadır.
