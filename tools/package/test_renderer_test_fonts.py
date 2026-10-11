import importlib.util
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "renderer_test_fonts", Path(__file__).with_name("renderer-test-fonts.py")
)
FIXTURE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(FIXTURE)


class RendererTestFontTests(unittest.TestCase):
    def test_makepkg_sources_match_ci_checksums_and_immutable_revision(self):
        result = subprocess.check_output(
            ["bash", "-c", 'source "$1"; printf "%s\\n" "${source[@]}"; '
             'printf "%s\\n" "${sha256sums[@]}"', "bash", str(ROOT / "packaging/PKGBUILD")],
            text=True,
        ).splitlines()
        self.assertEqual(len(result), 10)
        sources, sums = result[:5], result[5:]
        self.assertEqual(sums[0], "SKIP")
        ci = (ROOT / ".github/workflows/ci.yml").read_text()
        pins = {name: digest for digest, name in re.findall(
            r"([0-9a-f]{64})  (JetBrainsMonoNerdFont-\w+\.ttf)", ci
        )}
        self.assertEqual(len(pins), 4)
        base = re.search(r"base='([^']+)'", ci).group(1)
        for style, source, digest in zip(FIXTURE.STYLES, sources[1:], sums[1:]):
            name = f"JetBrainsMonoNerdFont-{style}.ttf"
            self.assertEqual(source, f"{base}/{style}/{name}")
            self.assertEqual(digest, pins[name])

    def test_private_fixture_escapes_paths_and_excludes_ambient_fonts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            source.mkdir()
            for style in FIXTURE.STYLES:
                (source / f"JetBrainsMonoNerdFont-{style}.ttf").write_bytes(style.encode())
            output = root / 'private & <font> "space"'
            config_path = FIXTURE.prepare(source, output)
            config = ET.parse(config_path).getroot()
            self.assertEqual(
                [node.text for node in config.findall("dir")],
                [str(output / "fonts"), *FIXTURE.FALLBACK_DIRS],
            )
            self.assertFalse(config.findall("include"))
            self.assertEqual(config.findtext("cachedir"), str(output / "cache"))
            self.assertEqual(config.findtext("alias/prefer/family"), "JetBrains Mono Nerd Font")
            for path in source.iterdir():
                self.assertEqual(path.read_bytes(), (output / "fonts" / path.name).read_bytes())
            before = config_path.read_bytes()
            self.assertEqual(FIXTURE.prepare(source, output).read_bytes(), before)

    def test_missing_source_fails_before_creating_fixture(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaisesRegex(FileNotFoundError, "missing checked font source"):
                FIXTURE.prepare(root, root / "output")
            self.assertFalse((root / "output").exists())

    def test_check_subshell_scopes_environment_and_propagates_fixture_failure(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            script = '''
set -e
source "$1"
export FONTCONFIG_FILE=ambient
srcdir=$2
mkdir "$srcdir/$pkgbase-$pkgver"
cd "$srcdir"
python() {
  if [[ $1 == tools/package/renderer-test-fonts.py ]]; then
    [[ $2 == "$srcdir" && $3 == "$srcdir/.splinterm-renderer-fonts" ]]
    printf '%s\\n' private-fixture
  fi
}
cargo() { [[ $FONTCONFIG_FILE == private-fixture ]]; }
check
[[ $FONTCONFIG_FILE == ambient ]]
python() { return 19; }
cargo() { touch "$srcdir/unexpected-cargo"; }
set +e
check
result=$?
[[ $result == 19 && ! -e $srcdir/unexpected-cargo ]]
'''
            subprocess.run(["bash", "-c", script, "bash", str(ROOT / "packaging/PKGBUILD"),
                            str(root)], check=True)


if __name__ == "__main__":
    unittest.main()
