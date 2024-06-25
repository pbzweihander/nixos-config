function ds
    argparse --min-args=1 'v/version=?' e/edit a/allow -- $argv; or return
    set -q argv[2]; or set argv[2] .
    cp -ir ~/nixos-config/devshells/$argv[1]/.* $argv[2]; or return
    if set -q _flag_v
        set _v (string sub --start 2 $_flag_v)
        sed -i "s/<version>/$_v/g" $argv[2]/.flake/flake.nix
    end
    if set -q _flag_e
        $EDITOR $argv[2]/.flake/flake.nix
    end
    if set -q _flag_a
        direnv allow $argv[2]
    end
end
