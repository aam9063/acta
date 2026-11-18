cask "acta" do
  version "0.1.1"
  sha256 "2259b8fc6521ce3c15d4e7eadbbbe61e7d393b09107ee20aaadf839c12b88624"

  url "https://github.com/edunavajas/acta/releases/download/v#{version}/acta-#{version}-macos-arm64.zip"
  name "acta"
  desc "Grabador de reuniones local: transcribe en tu Mac y redacta el acta"
  homepage "https://github.com/edunavajas/acta"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on arch: :arm64
  depends_on macos: ">= :sonoma"

  app "acta.app"

  zap trash: [
    "~/Library/Application Support/acta",
    "~/Library/Caches/com.edunavajas.acta",
    "~/Library/Preferences/com.edunavajas.acta.plist",
    "~/Library/WebKit/com.edunavajas.acta",
  ]

  caveats <<~EOS
    acta se distribuye sin firmar ni notarizar, así que macOS la bloqueará la primera vez.
    Para abrirla: Ajustes del Sistema › Privacidad y seguridad › "Abrir igualmente".
    Alternativa en terminal:
      xattr -dr com.apple.quarantine "#{appdir}/acta.app"
  EOS
end
