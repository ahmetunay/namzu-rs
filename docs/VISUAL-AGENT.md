# Görsel ajan araçları — 4.4

SDK `GuestTools::with_visual(VisualPolicy)` artık owned bilgisayarın masaüstü ve
tarayıcı araçlarını normal ajan döngüsüne açar. Varsayılan görsel izin yoktur.
Açık izin, bilgisayarın gerçek donanımı ve genel tool allow/deny listesi birlikte
uygulanır; deny önceliklidir. Hatalı batch ilk etkiden önce doğrulanır.
Host ekranı, host tarayıcısı veya başarısız guest yerine host shell kullanılmaz.

## CLI ve TUI

Önce `python3 scripts/build_browser_image.py` ile yerel imajı derle. Sabit imaj
kimliği `artifacts/browser-image.json` içindedir; tag kabul edilmez. Örnek:

```sh
./target/release/namzu-rs exec 'Sayfayı incele' \
  --model GORSEL_DESTEKLI_MODEL --api-key-env OPENAI_API_KEY \
  --computer-image sha256:IMAJIN_64_HEX_KIMLIGI \
  --computer-browser --visual-read --visual-navigate --visual-actions --json
```

Aynı seçenekler `tui` için geçerlidir. `--computer-browser` masaüstünü de açar;
yalnız ekran/girdi için `--computer-desktop` seçilir. İkisi de açık
`--computer-image` gerektirir. Genel dosya/shell araçları bu durumda guest içinde
çalışır; `--allow-shell` ve `--allow-write` yine ayrıca gerekir.

| İzin | Araçlar |
| --- | --- |
| `--visual-read` | `computer_info`, `computer_screenshot`, `browser_info`, `browser_snapshot` |
| `--visual-input` | `computer_input`: koordinat, fare, scroll, key ve ASCII type |
| `--visual-navigate` | `browser_navigate` |
| `--visual-actions` | `browser_action`: click, Unicode fill, select, press, scroll_into_view |
| `--visual-upload` | `browser_upload`: guest dosyasını file input'a ekle |
| `--visual-download` | `browser_download`: yeni guest dosyasına indir |

Salt okunur görsel araçlar TUI'de otomatik onaylanır. Diğer araçlar mevcut izin
tavanı içinde ayrıca tek kullanımlık y/n onayı ister. F7 son ekran görüntüsünü
renkli terminal önizlemesiyle açar/kapatır; Esc görüntüden geri döner. Görüntü
üstteyken y/n etkisizdir; Ctrl+C aktif görevi iptal eder. Önizleme terminale göre
ölçeklenir; modele verilen PNG ve koordinatları tam boyutludur. Kaydedilmiş özel
oturum açıldığında son görüntü geri yüklenir; yeni konuşmada temizlenir.

## Görüntü sözleşmesi ve model sağlayıcıları

`ToolResult.image: Option<ToolImage>` ile gerçek RGB8 PNG taşınır. Boyut, base64,
PNG formatı/pikselleri/CRC ve sonlandırma doğrulanır; en çok 1920×1080 ve 1 MiB
sıkıştırılmış PNG. Büyük görüntü otomatik küçültülmez; araç hatası döner. Core,
başarılı `Tool` mesajının hemen ardına aynı çağrı ID'siyle `ToolImage` kaydeder;
yetim, yinelenmiş, bozuk veya hata sonucuna bağlı görüntü reddedilir.
Görseller de varsayılan 4 MiB konuşma bütçesinden tüketir; dolunca yeni sağlayıcı
çağrısı yapılmaz. SDK event/journal/history görüntüyü korur.

OpenAI uyumlu Chat Completions sürücüsü `image_url`, native Codex Responses
sürücüsü `input_image` üretir. Wire üzerinde bütün function sonuçları tamamlanır,
ardından görüntü ayrı user multipart verisi olarak eklenir; kaynak çağrı ID'si
ve açık “untrusted image” açıklaması bulunur. Görsel veya sayfa içeriği
system talimatına dönüştürülmez. Tool sonucunun metnine base64 gömülmez.
Modelin bu içerik türünü desteklemesi gerekir; sessiz text fallback yoktur.
Bu aşamanın kabulünde canlı görsel model veya abonelik kotası kullanılmadı.
Özel session dosyaları PNG içerebilir; kaynak depoya yüklenmez.

## Yaşam döngüsü ve Pals

Normal exec/TUI tamamlanan turda ephemeral bilgisayarı ve volume'u temizler.
Resume, sağlayıcı/MCP/imaj/donanım rotasını korur; sonraki tur yeni bilgisayar ve
sayfa profiliyle başlar. Önceki sayfa hedefleri yeniden kullanılmamalıdır.
Belirsiz etkide bilgisayar durur, workspace ve pending kaydı inceleme için kalır;
otomatik tekrar/ack yoktur. Açık işletmen komutları:

```sh
./target/release/namzu-rs session --state-dir STATE computer SESSION_ID show
./target/release/namzu-rs session --state-dir STATE computer SESSION_ID read GUEST_PATH
./target/release/namzu-rs session --state-dir STATE computer SESSION_ID cleanup
```

`read` en çok 64 KiB base64 JSON döndürür, pending kaydını kaldırmaz; `cleanup`
yalnız ilgili owned kaynakları siler. Devam eden çalışma lock ile korunur.

Kalıcı Pal örneği (aynı provider/model/key seçenekleri her komuta eklenebilir):

```sh
namzu-rs pal --model MODEL --guest --visual-read --visual-actions create Browser
namzu-rs pal --image sha256:IMAJ_KIMLIGI computer PAL_ID create --desktop --browser
namzu-rs pal --visual-read --visual-actions ask PAL_ID 'Sayfayı incele'
namzu-rs pal computer PAL_ID stop
namzu-rs pal computer PAL_ID cleanup
```

Pal oluşturulurken izinli tool adları profil tavanına kaydedilir. Her görevde
ilgili `--visual-*` izinleri yeniden gerekir; sonraki invocation bayrakları
profil tavanını genişletemez. Alt ajan delegasyonu mevcut tool kesişimini
korur; read-only varsayılanı etki araçları vermez. Sağlıklı kalıcı görsel Pal
turundan sonra owned bilgisayar çalışır durumda kalır; aynı Chromium belge
kimliği sonraki görevde korunur. Hata/iptalde durur, pending varsa korunur.
Kullanım bitince açık stop/cleanup gerekir. Ephemeral `pal --computer-image`
tur sonunda temizlenir. Guest MCP stdio kullanımı mevcut join/stop korumasını
korur; MCP kapanışı bilgisayarı durdurduğu için sonraki turda tarayıcı profili
sıfırlanır. `/tmp` Chromium profili kalıcı disk profili değildir.

## Kabul

- 439 Rust testi: PNG doğrulaması ve history/bütçe, iki sağlayıcının multipart
  sırası, donanım/izin kesişimi, izin sınıfı kaçaklarının reddi, batch ön kontrolü,
  renkli TUI history önizlemesi ve mevcut modül regresyonları.
- Format, workspace test, Clippy `-D warnings`, release derlemesi.
- `python3 scripts/visual_agent_smoke.py`: gerçek Docker/Chromium + loopback
  sağlayıcı; Unicode form/tıklama, DOM oracle, gerçek PNG multipart; Linux PTY
  görüntü/onay/ret/iptal ve terminal geri yüklemesi; stale hedefte durma,
  pending korunarak işletmen okuması/temizlik, backend değiştirerek resume reddi;
  Pal profil tavanı ve iki tur arasında aynı sayfanın korunması.
- `python3 scripts/browser_actions_smoke.py`: 4.2/4.3 gerçek SDK/CLI regresyonu.

Yerel site fixture'ı ajan çağrılarını deterministik seçer. Bu sonuç model
zekâsı/vision kalitesi veya genel internette tarayıcı uyumluluğu ölçümü değildir.
Mevcut offline tek sekme, main document ve 64 KiB dosya sınırları sürer;
iframe, pop-up, kalıcı browser profili ve genel host bilgisayar kontrolü yoktur.
