# TUI 1.1–1.4 kabul kaydı — 2026-10-17

SDK olay akışı ve native terminal mesaj ekranı dört parça halinde yeniden
uygulandı. Aşağıdaki eski aşama sayıları kendi teslimlerine aittir. Son kaynak
kabulü: 103 Rust testi, 25 Linux PTY senaryosu, format/Clippy/release ve ayrı
GPT-6-Luna canlı okuma görevi geçti. TUI planının dört parçası tamam; Pals/MCP içermez.

## 1.1 kaynak kontrolleri

- Workspace: **93 test geçti, 0 başarısız, 0 atlandı**.
- `cargo fmt --all -- --check`: geçti.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: geçti.
- `cargo build --release --workspace --locked`: geçti.
- Önceki exec/session/benchmark testleri dâhil; yeni testler SDK olay sırası,
  pending→tool start→etki→checkpoint→tool finish, kopan tüketicide etkiyi engelleme,
  geçici delta/bozuk batch, SSE metin/reasoning ayrımı, olay teslim süresinin bütçeye
  katılması, Unicode/render, görüntü sınırları ve hata sonrası açık yeni konuşma.

## 1.1 optimize binary ile gerçek PTY

| Kontrol | Sonuç |
| --- | --- |
| Offline demo: dosya yaz/oku ve 12 yanıtı; auth/state erişimi yok | Geçti |
| Loopback HTTP: read_file sonucu → yanıt → ikinci mesajda kalıcı session resume | Geçti |
| Eksik sağlayıcı yanıtı: tamamlandı görünümü yok, otomatik tekrar yok; Ctrl-N yeni session | Geçti |
| Shell çalışırken Ctrl-C: çocuk kapandı, ikinci write_file çalışmadı | Geçti |
| Shell çalışırken Esc: worker birleştirildi, çocuk kapandı, ikinci araç çalışmadı | Geçti |

Her senaryoda çıkış kodu, terminal termios ayarlarının başlangıçla birebir eşitliği,
alternate screen'den çıkış ve bracketed paste'in kapanışı kontrol edildi. Süreç
çıkışı Linux pidfd ile doğrulandı; rastgele sleep sonrası PID yokluğu varsayılmadı.
Test metni gerçek VT hücrelerinden okunur; ANSI kaçışlarının ham string eşleşmesine
dayanmaz. TUI olmayan pipe çağrısının model/state açmadan reddi ayrıca sınandı.

## 1.1 canlı Codex kabulü

Önceden yetkilendirilmiş hesaptan GPT-6-Luna (`gpt-6-luna`), low effort ve 60 saniye
bütçeyle gerçek PTY içinde tek read-only görev çalıştırıldı. Ajan yalnızca read_file
aracıyla geçici input.txt içeriğini okudu; TUI'de araç ve beklenen yanıt görüldü.
Turn Ready durumuna döndü ve çıkışta terminal ayarları geri geldi. Shell/yazma
izni verilmedi. Normal Rust/PTY fixture testleri bu hesap çağrısını yapmaz.

Token, hesap kimliği, auth/session dosyaları ve ham ekran/model içerikleri bu
belgeye veya GitHub'a eklenmedi. Bu bir performans karşılaştırması değildir;
önceki Luna benchmark'ı kendi kaynak commit'i ve raporuyla ayrı tutulur.

1.1 aşamasında bloklayan HTTP okumasında iptal istek deadline'ını bekleyebiliyordu.
Bu kısıt aşağıdaki 1.3 tesliminde giderildi. Güncel sınırlar: [TUI.md](TUI.md).

## 1.2 — editör ve araç ayrıntıları

96 Rust testi, format, Clippy ve release geçti. Altı gerçek PTY senaryosu geçti;
yeni senaryoda F2 ile çok satırlı gerçek prompt sunucuya birebir ulaştı ve diff
kartı Tab ile açılınca son satır göründü. Unicode düzenleme/sarma ve aynı ID'li
araç kartının sonuçla güncellenmesi ayrıca sınandı. Bu aşamada canlı model çağrılmadı.

## 1.3 — onay ve hızlı iptal

99 Rust testi, format, Clippy ve release geçti. 13 gerçek PTY senaryosu geçti.
Yeni kontroller: bir çağrıyı onaylayıp sonraki çağrıyı reddetme, ilk çağrıyı ret,
onay beklerken iptal, CLI izin tavanını aşan isteğin hiç onay sormadan reddi;
HTTP başlık/gövde beklerken Ctrl-C ve gövde beklerken Esc. Socket'in kapandığı ve
etkinin oluşmadığı kontrol edildi. Async future'ın gerçekten poll edildikten
sonra iptal edilip düşürüldüğü deterministik bir kanal bariyeriyle de sınandı.
Core onay hook'u journal ve efekt sınırlarını korur. Önceki 1.1'deki bloklayan
HTTP iptal kısıtı bu aşamada kaldırıldı; OS/kernel I/O kısıtları geçerlidir.

## 1.4 — oturum, model/bütçe ve terminal yaşam döngüsü

**103 Rust testi**, format, Clippy (`-D warnings`) ve optimize release build geçti.
**25 gerçek PTY senaryosu** geçti. 1.3'e eklenen kontroller:

- F5 ile kayıtlı geçmişi açıp aynı session'a ikinci mesaj gönderme; önceki cevap
  gerçek sağlayıcı HTTP isteğinin geçmişinde korundu.
- Failed session'ı görüntüleyip gönderimin engellenmesi; Ctrl-N açık yeni session.
- F6 model/bütçe ayarlarını uyguladıktan sonra gerçek HTTP isteğinde yeni model;
  geçersiz ayarı reddedince önceki modelin korunması.
- Byte bütçesi küçük seçilince HTTP başlamadan, adım bütçesi 1 seçilince ikinci
  model isteğinden önce durma. İzinler ayar paneliyle artırılmadı.
- HTTP gövdesi beklerken SIGTERM; SIGHUP; küçük terminale resize ve geri dönüş;
  ayar/oturum panelinde Ctrl-C. Menüye yapıştırılan metin ana girişe sızmadı.
- Her çıkışta termios başlangıç değerleri, alternate screen ve bracketed paste
  geri yüklendi. Soket kapanışı ayrıca Event bariyeriyle alındı.

Ayrı bir GPT-6-Luna, low effort, 60 saniye read-only gerçek PTY göreviyle yeni
async HTTP/TUI yolunda dosya aracı, tamamlanan yanıt ve terminal temizliği de
canlı doğrulandı. Model/hesap erişimi yeniden login istemeden mevcut yetkilendirmeyi
kullandı. Normal testler canlı kota harcamaz.

TUI ayarları yalnızca süreç içinde tutulur. Session listesi aynı workspace'teki
son 256 dosya adayıyla sınırlıdır; eski ID `--resume` ile açık seçilebilir.
Başka rota/hesap otomatik benimsenmez, belirsiz etkiler tekrarlanmaz. OS/kernel
I/O'su veya SIGKILL için anlık iptal/cleanup garantisi yoktur. Kabul Linux içindir.

| Teslim | Rust testleri | PTY senaryoları | Kaynak commit |
| --- | --- | --- | --- |
| 1.1 | 93 | 5 | 91c04d8 |
| 1.2 | 96 | 6 | a0f681d |
| 1.3 | 99 | 13 | e66895e |
| 1.4 | 103 | 25 | Bu kayıtla birlikte yayımlanır |
