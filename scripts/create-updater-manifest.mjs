import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { basename, resolve } from 'node:path';

const [version, platform, artifact, output = 'latest.json'] = process.argv.slice(2);

if (!version || !platform || !artifact) {
  console.error('Aufruf: npm run make-updater-manifest -- <version> <platform> <artefakt> [ausgabe]');
  console.error('Beispiel: npm run make-updater-manifest -- 0.4.33 darwin-aarch64 "…/DualBeam.app.tar.gz"');
  process.exit(1);
}

if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`Keine gültige semantische Version: ${version}`);
  process.exit(1);
}

const artifactPath = resolve(artifact);
if (!existsSync(artifactPath)) {
  console.error(`Updater-Archiv nicht gefunden: ${artifactPath}`);
  console.error('Wurde "createUpdaterArtifacts" in tauri.conf.json aktiviert und der Build ausgeführt?');
  process.exit(1);
}

const signaturePath = `${artifactPath}.sig`;
if (!existsSync(signaturePath)) {
  console.error(`Signaturdatei nicht gefunden: ${signaturePath}`);
  console.error('Ohne TAURI_SIGNING_PRIVATE_KEY_PATH erzeugt der Build keine Signatur.');
  process.exit(1);
}

const signature = readFileSync(signaturePath, 'utf8').trim();
if (!signature) {
  console.error(`Signatur ist leer: ${signaturePath}`);
  process.exit(1);
}

const assetName = basename(artifactPath);
const manifest = {
  version,
  notes: `DualBeam ${version}`,
  pub_date: new Date().toISOString(),
  platforms: {
    [platform]: {
      signature,
      url: `https://github.com/nojan01/macos-dualpane/releases/download/v${version}/${encodeURIComponent(assetName)}`
    }
  }
};

const outputPath = resolve(output);
writeFileSync(outputPath, `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`Updater-Manifest erstellt: ${outputPath}`);
