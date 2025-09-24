{ pkgs, ... }:

pkgs.stdenv.mkDerivation {
  name = "boundless-aux";
  version = "1.0.1";

  # Download prebuilt binaries from GitHub releases
  src = pkgs.fetchurl {
    url = "https://github.com/boundless-xyz/boundless/releases/download/bento-v1.0.1/bento-bundle-linux-amd64.tar.gz";
    sha256 = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="; # Update with actual hash
  };

  nativeBuildInputs = with pkgs; [
    gzip
    tar
  ];

  unpackPhase = ''
    tar -xzf $src
  '';

  installPhase = ''
    mkdir -p $out/bin
    cp agent $out/bin/
    chmod +x $out/bin/agent
  '';
}
