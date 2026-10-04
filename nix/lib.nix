let
  readPack = src: builtins.fromJSON (builtins.readFile (src + "/riven.json"));

  mkModpack =
    {
      pkgs,
      src,
      side ? "server",
      groups ? { },
      exclude ? [ ],
    }:
    let
      inherit (pkgs) lib;
      pack = readPack src;
      groupDefault =
        g: (lib.findFirst (x: x.id == g) { default = false; } (pack.groups or [ ])).default or false;
      groupOn = g: groups.${g} or (groupDefault g);
      wanted =
        e:
        (e.side == "both" || e.side == side)
        && !(builtins.elem e.id exclude)
        && ((e.group or null) == null || groupOn e.group);
      # Store names only allow [A-Za-z0-9+._?=-]; pack file names can hold anything.
      storeName = e: lib.strings.sanitizeDerivationName (baseNameOf e.file.path);
      fetch =
        e:
        if e.source.type == "local" then
          builtins.path {
            path = src + "/${e.source.path}";
            name = storeName e;
          }
        else
          pkgs.fetchurl (
            {
              url = e.file.url;
              name = storeName e;
            }
            // (
              if e.file.hashes ? sha512 then
                { sha512 = e.file.hashes.sha512; }
              else
                { sha1 = e.file.hashes.sha1; }
            )
          );
      entries = builtins.filter wanted (pack.content or [ ]);
      ignored = (pack.files or { }).ignore or [ ];
      # riven's globs: `*`/`?` stay within a segment, `**` spans segments.
      globRegex =
        glob:
        let
          escaped = lib.escapeRegex glob;
          parts = lib.splitString "\\*\\*" escaped;
          segment = p: lib.replaceStrings [ "\\*" "\\?" ] [ "[^/]*" "[^/]" ] p;
        in
        lib.concatStringsSep ".*" (map segment parts);
      ignoreRegex = lib.concatStringsSep "|" (map (g: "(${globRegex g})") ignored);
    in
    pkgs.runCommand "${pack.id}-${pack.version}-${side}"
      {
        passthru = { inherit pack; };
        nativeBuildInputs = [ pkgs.findutils ];
      }
      ''
        mkdir -p $out
        ${lib.concatMapStrings (
          e: "install -D -m 444 ${fetch e} $out/${lib.escapeShellArg e.file.path}\n"
        ) entries}
        for d in common ${side}; do
          dir=${src}/overrides/$d
          [ -d "$dir" ] || continue
          (cd "$dir" && find . -type f -printf '%P\0') | while IFS= read -r -d "" rel; do
            ${lib.optionalString (ignored != [ ]) ''
              if printf '%s' "$rel" | grep -Eq ${lib.escapeShellArg "^(${ignoreRegex})$"}; then
                continue
              fi
            ''}
            install -D -m 644 "$dir/$rel" "$out/$rel"
          done
        done
      '';
in
{
  inherit readPack mkModpack;
}
