{ pkgs }:
let
  python = pkgs.python3.withPackages (p: [ p.boto3 ]);
  mkTool = command: description: pkgs.writeShellApplication {
    name = "plow-smi-${command}";
    runtimeInputs = [ pkgs.git pkgs.gh ];
    text = ''exec ${python}/bin/python3 ${./release_tools.py} ${command} "$@"'';
    meta = { inherit description; };
  };
in {
  packages = {
    bump = mkTool "bump" "Bump the Cargo workspace and lockfile version";
    release-version = mkTool "release-version" "Select or resume a release version";
    release-tag = mkTool "release-tag" "Create or verify the release tag";
    publish-pdist = mkTool "publish-pdist" "Publish complete immutable archives to R2";
    publish-github = mkTool "publish-github" "Verify and publish GitHub release assets";
  };
  check = pkgs.runCommand "plow-smi-release-ci-tests"
    { nativeBuildInputs = [ python pkgs.git ]; }
    ''
      export HOME="$TMPDIR" PYTHONDONTWRITEBYTECODE=1
      cp ${./release_tools.py} release_tools.py
      cp ${./test_release_tools.py} test_release_tools.py
      python3 -m unittest -v test_release_tools
      touch "$out"
    '';
}
