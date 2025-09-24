{ pkgs, ... }:

pkgs.stdenv.mkDerivation {
  name = "boundless-broker";
  version = "1.0.0";

  # Download prebuilt broker binary from GitHub releases
  src = pkgs.fetchurl {
    url = "https://github.com/boundless-xyz/boundless/releases/download/broker-v1.0.0/broker";
    sha256 = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="; # Update with actual hash
  };

  nativeBuildInputs = with pkgs; [
    # No build inputs needed for prebuilt binary
  ];

  unpackPhase = ''
    # No unpacking needed for single binary
  '';

  installPhase = ''
    mkdir -p $out/bin
    cp $src $out/bin/broker
    chmod +x $out/bin/broker
  '';
}
