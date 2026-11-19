cask "acta" do
  version "0.1.2"
  sha256 "20d18009a7b28b2f08832c95639274b199353cde9db70d0afd3903ea96d76a2e"

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
