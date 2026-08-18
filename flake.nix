{
  description = "nyth: write-through OverlayFS runtime for Home Manager";

  inputs = {
    nixpkgs.url = "nixpkgs";
  };

  outputs = { self, nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      pkgsFor = system: nixpkgs.legacyPackages.${system};
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = pkgsFor system;
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "nyth";
            version = "0.1.0";
            src = self;

            cargoLock.lockFile = ./Cargo.lock;

            # Test suite needs CAP_SYS_ADMIN for real mounts; sandbox has none.
            doCheck = false;

            meta = {
              description = "Write-through OverlayFS runtime for Home Manager";
              license = pkgs.lib.licenses.gpl3Plus;
              platforms = pkgs.lib.platforms.linux;
              mainProgram = "nyth";
            };
          };
        });

      homeManagerModules.default = import ./modules/nyth.nix { inherit self; };

      devShells = forAllSystems (system:
        let
          pkgs = pkgsFor system;
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [ cargo rustc rust-analyzer clippy rustfmt ];
          };
        });

      formatter = forAllSystems (system: (pkgsFor system).nixfmt-rfc-style);
    };
}
