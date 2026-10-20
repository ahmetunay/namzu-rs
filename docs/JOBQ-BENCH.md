# Jobq — checker öncesi kör teslim, 2026-10-20

Durum: **iki ajan tamamladı; bağımsız doğruluk değerlendirmesi checker.zip bekliyor**.
Exit 0 veya ajanın kendi test beyanı görev başarısı sayılmadı. Gönderilen task
paketi iki ayrı temiz workspace'e aynı dosya hash'leriyle açıldı. Aynı prompt,
GPT-6-Luna/low ve 300 saniye bütçe kullanıldı; birer koşu yapıldı. Rastgele sıra
Jcode → Namzu-RS; seed ve zaman damgaları [sayısal kayıtta](../bench/jobq-pending-2026-10-20.json).
Bu tek tekrar ABBA veya tekrarlı sıra dengelemesi değildir.

| Ölçüm | Namzu-RS | Jcode |
| --- | --- | --- |
| Süre | 38.942 s | 77.468 s |
| wait4 bireysel high-water RSS | 14.75 MiB | 46.62 MiB |
| Örneklenen süreç ağacı peak RSS toplamı | 28.02 MiB | 70.13 MiB |
| Süreç exit code | 0 | 0 |
| Görünür test dosyaları | Değişmedi | Değişmedi |
| Bağımsız görev doğruluğu | Checker bekliyor | Checker bekliyor |

Her iki ajan yalnızca jobq/cli.py, jobq/jobqueue.py ve jobq/store.py değiştirdi.
Kod veya agent yanıtları görev doğruluğu açısından burada değerlendirilmedi;
çekirdek hash kontrolü yalnızca testlerin korunması ve teslim bütünlüğü içindir.
Çözümlere çalışma bittikten sonra müdahale edilmedi.

Süreç ağacı ölçümü: 100 ms aralıklarla root ve gözlenen recursive çocukların
RSS toplamı. Paylaşılan sayfalar birden fazla sayılabilir; kısa pikler ve yeniden
parent edilen/kaçan çocuklar gözden kaçabilir. PSS veya tam OS izolasyonu değildir.
Önceki wait4 peak ölçümüyle aynı metrik diye birleştirilmemeli.

Namzu'nun token verisi SDK toplamları ve alan başına samples kapsamını içerir.
Jcode'un CLI usage alanları olduğu gibi kaydedildi; bütün model isteklerini
kapsadığı doğrulanmadı. Bu iki şema eşdeğer toplam token tüketimi diye
karşılaştırılmaz. İlk model isteğine kadar süre iki tarafta eşdeğer biçimde
ölçülmediği için null; tahminle doldurulmadı. Bu görevde ek READY turu yapılmadı.

Teslimler ayrı ZIP ve dosya SHA-256 manifestleriyle checker gelmeden sabitlendi.
Kaynak/input ZIP'leri, özel ham çıktılar ve runner cloud artifacts'te tutulur;
bu public depoya görev kaynakları, çözümler veya özel agent logları yüklenmedi.
Ölçüm raporu ve arşiv hash'leri bu depoda yedeklendi. Kullanıcıya sunulan
jobq-frozen-results.zip paketi input ve iki çözüm arşivi, manifestler, rapor ve
ölçüm runner'ını içerir. ZIP/bütünlük kanıtı cloud artifacts'tedir.

Paket SHA-256: `3dcba1898d495fc8dd888419abca1986ca3cbaa1ac488974465c6cd0c895e230`.
Checker geldiğinde ZIP hash'leri tekrar doğrulanıp çözümlerin yeni açılmış
kopyaları sınanacak; dondurulmuş teslimler ve bu ilk ölçüm kaydı korunacak.
