function ds
    argparse --min-args=1 'v/version=?' e/edit a/allow -- $argv; or return
    cp -ir ~/nixos-config/devshells/$argv[1]/.* .; or return
    sed -i "s/<path>/$(pwd | sed 's/\\//\\\\\\//g')/g" .envrc; or return
    sed -i "s/<nixos-ref>/$(jaq -r '.nodes.nixpkgs.original.ref' $NH_FLAKE/flake.lock)/g" .flake/flake.nix; or return
    if set -q _flag_v
        set _v (string sub --start 2 $_flag_v)
        sed -i "s/<version>/$_v/g" .flake/flake.nix; or return
    end
    if set -q _flag_e
        $EDITOR .flake/flake.nix
    end
    if set -q _flag_a
        direnv allow
    end
end
