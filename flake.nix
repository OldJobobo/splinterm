{
  description = "Splinterm — persistent Wayland terminal and headless session daemon";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";

  outputs =
    { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          package = pkgs.callPackage ./nix/package.nix {
            src = ./.;
            sourceRevision =
              self.rev or (if self ? dirtyRev then nixpkgs.lib.removeSuffix "-dirty" self.dirtyRev else null);
          };
        in
        {
          default = package;
          splinterm = package;
          splinterm-with-mcp = package.override { withMcp = true; };
        }
      );

      nixosModules.default = import ./nix/module.nix { inherit self; };
      nixosModules.splinterm = self.nixosModules.default;

      checks = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          package = self.packages.${system}.default;
          module = import ./nix/module-check.nix {
            inherit
              self
              nixpkgs
              pkgs
              system
              ;
          };
        }
      );
    };
}
