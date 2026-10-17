# TUI 1.1 kabul kaydı — 2026-10-17

SDK olay akışı ve native terminal mesaj ekranı yeniden uygulandı. Bu kabul
TUI'nin dört parçalık planının yalnızca 1.1 teslimidir; Pals/MCP içermez.

## Kaynak kontrolleri

- Workspace: **93 test geçti, 0 başarısız, 0 atlandı**.
- `cargo fmt --all -- --check`: geçti.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: geçti.
- `cargo build --release --workspace --locked`: geçti.
- Önceki exec/session/benchmark testleri dâhil; yeni testler SDK olay sırası,
  pending→tool start→etki→checkpoint→tool finish, kopan tüketicide etkiyi engelleme,
  geçici delta/bozuk batch, SSE metin/reasoning ayrımı, olay teslim süresinin bütçeye
  katılması, Unicode/render, görüntü sınırları ve hata sonrası açık yeni konuşma.

## Optimize binary ile gerçek PTY

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

## Canlı Codex kabulü

Önceden yetkilendirilmiş hesaptan GPT-6-Luna (`gpt-6-luna`), low effort ve 60 saniye
bütçeyle gerçek PTY içinde tek read-only görev çalıştırıldı. Ajan yalnızca read_file
aracıyla geçici input.txt içeriğini okudu; TUI'de araç ve beklenen yanıt görüldü.
Turn Ready durumuna döndü ve çıkışta terminal ayarları geri geldi. Shell/yazma
izni verilmedi. Normal Rust/PTY fixture testleri bu hesap çağrısını yapmaz.

Token, hesap kimliği, auth/session dosyaları ve ham ekran/model içerikleri bu
belgeye veya GitHub'a eklenmedi. Bu bir performans karşılaştırması değildir;
önceki Luna benchmark'ı kendi kaynak commit'i ve raporuyla ayrı tutulur.

Sınırlar ve kullanım: [TUI.md](TUI.md). Özellikle bloklayan HTTP okumasında iptal
istek deadline'ına kadar bekleyebilir; hızlı iptal ve ayrıntılı onaylar 1.3'tedir.

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
