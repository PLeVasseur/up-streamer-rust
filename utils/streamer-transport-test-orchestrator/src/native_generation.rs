// SPDX-License-Identifier: Apache-2.0
//! Immutable evidence for application-linked native agreement generation.

use super::*;
use up_native_profile::{
    AgreementArtifact, AllocationBand, IdentityDeclarations, InventoryArtifact, SeedAllocation,
};

const GENERATOR: &str = "streamer-native-inventory";
const FILES: [&str; 6] = [
    "inventory.json",
    "agreement.json",
    "seeds.json",
    "identities.json",
    "stdout.log",
    "stderr.log",
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeGeneration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    single_type: Option<up_native_profile::TypeKey>,
    inventory: InventoryArtifact,
    seeds: Vec<SeedAllocation>,
    identities: IdentityDeclarations,
    command: Vec<String>,
    exporter_sha256: String,
    previous_agreement_sha256: Option<String>,
    file_sha256: BTreeMap<String, String>,
}

fn command(target: &str) -> Vec<String> {
    [
        "bin/streamer-native-inventory",
        "--output",
        "native/inventory.json",
        "--agreement-output",
        "native/agreement.json",
        "--expect-target",
        target,
        "--domain",
        "uprotocol.examples.native",
        "--seeds",
        "native/seeds.json",
        "--identities",
        "native/identities.json",
    ]
    .map(str::to_owned)
    .to_vec()
}

fn command_for_type(target: &str, single_type: Option<&up_native_profile::TypeKey>) -> Vec<String> {
    let mut args = command(target);
    if let Some(key) = single_type {
        args.extend(["--single-type".into(), key.name.clone()]);
    }
    args
}

impl NativeGeneration {
    pub(super) fn generate(
        root: &Path,
        exporter_sha256: &str,
    ) -> Result<(NativeProfileDocument, Self)> {
        Self::generate_for_type(root, exporter_sha256, None)
    }

    pub(super) fn generate_for_type(
        root: &Path,
        exporter_sha256: &str,
        type_name: Option<&str>,
    ) -> Result<(NativeProfileDocument, Self)> {
        let directory = root.join("native");
        fs::create_dir(&directory)?;
        let seeds = matrix_seed_allocations();
        let identities = matrix_identity_declarations();
        up_native_profile::write_artifact(&directory.join("seeds.json"), &seeds)?;
        up_native_profile::write_artifact(&directory.join("identities.json"), &identities)?;
        let single_type = type_name.map(|name| up_native_profile::TypeKey {
            name: name.into(),
            revision: 0,
        });
        let command = command_for_type(
            up_rust::NativeTarget::current().triple(),
            single_type.as_ref(),
        );
        let output = Command::new(root.join(&command[0]))
            .args(&command[1..])
            .current_dir(root)
            .output()?;
        fs::write(directory.join("stdout.log"), &output.stdout)?;
        fs::write(directory.join("stderr.log"), &output.stderr)?;
        if !output.status.success() {
            return Err(anyhow!(
                "application native exporter failed; see {}",
                directory.display()
            ));
        }
        let inventory = up_native_profile::read_artifact(&directory.join("inventory.json"))?;
        let artifact = up_native_profile::read_artifact(&directory.join("agreement.json"))?;
        let mut file_sha256 = BTreeMap::new();
        for name in FILES {
            let path = directory.join(name);
            file_sha256.insert(name.into(), sha256_file(&path)?);
            fs::set_permissions(path, fs::Permissions::from_mode(0o444))?;
        }
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o555))?;
        let generation = Self {
            single_type,
            inventory,
            seeds,
            identities,
            command,
            exporter_sha256: exporter_sha256.into(),
            previous_agreement_sha256: None,
            file_sha256,
        };
        let profile = NativeProfileDocument(artifact);
        generation.validate(&profile)?;
        Ok((profile, generation))
    }

    pub(super) fn validate(&self, profile: &NativeProfileDocument) -> Result<()> {
        self.inventory.validate()?;
        profile.0.validate()?;
        if self.previous_agreement_sha256.is_some()
            || self.command
                != command_for_type(&self.inventory.target.triple, self.single_type.as_ref())
            || self.seeds != matrix_seed_allocations()
            || self.identities != matrix_identity_declarations()
            || self
                .file_sha256
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                != BTreeSet::from(FILES)
        {
            return Err(anyhow!(
                "native generation inputs or command differ from the explicit deployment policy"
            ));
        }
        validate_sha256("native exporter", &self.exporter_sha256)?;
        for hash in self.file_sha256.values() {
            validate_sha256("native generation file", hash)?;
        }
        let regenerated = if let Some(key) = &self.single_type {
            anyhow::ensure!(key.revision == 0, "exporter requires revision zero");
            AgreementArtifact::generate_single_type(
                &self.inventory,
                "uprotocol.examples.native".into(),
                key,
                self.identities.clone(),
            )?
        } else {
            AgreementArtifact::generate_table(
                &self.inventory,
                "uprotocol.examples.native".into(),
                1,
                AllocationBand::default(),
                None,
                &self.seeds,
                self.identities.clone(),
            )?
        };
        if regenerated != profile.0 {
            return Err(anyhow!("bundled agreement differs from the recorded target inventory and generation inputs"));
        }
        profile
            .0
            .require_wire::<up_rust::StableContainerWireFormat, up_rust::NativePrefixFrameMetadataCodec>()?;
        Ok(())
    }

    pub(super) fn validate_files(&self, bundle: &RunBundle) -> Result<()> {
        let exporter = bundle
            .manifest
            .files
            .iter()
            .find(|file| file.kind == BundleFileKind::MatrixExecutable && file.name == GENERATOR)
            .context("bundle is missing the actual native exporter")?;
        if exporter.sha256 != self.exporter_sha256 {
            return Err(anyhow!(
                "native generation exporter hash differs from bundled executable"
            ));
        }
        let directory = bundle.root.join("native");
        validate_bundle_mode(&directory, 0o555, true)?;
        validate_directory_entries(&directory, &self.file_sha256.keys().cloned().collect())?;
        for (name, hash) in &self.file_sha256 {
            let path = directory.join(name);
            validate_bundle_mode(&path, 0o444, false)?;
            if sha256_file(&path)? != *hash {
                return Err(anyhow!("native generator file changed: {name}"));
            }
        }
        let inventory: InventoryArtifact =
            up_native_profile::read_artifact(&directory.join("inventory.json"))?;
        let agreement: AgreementArtifact =
            up_native_profile::read_artifact(&directory.join("agreement.json"))?;
        let seeds: Vec<SeedAllocation> =
            up_native_profile::read_artifact(&directory.join("seeds.json"))?;
        let identities: IdentityDeclarations =
            up_native_profile::read_artifact(&directory.join("identities.json"))?;
        if inventory != self.inventory
            || agreement != bundle.manifest.native_profile.0
            || seeds != self.seeds
            || identities != self.identities
        {
            return Err(anyhow!(
                "native generator files disagree with the bundle manifest"
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn stage_fixture(root: &Path, exporter_sha256: &str) -> Self {
        let mut fixture = Self::fixture();
        fixture.exporter_sha256 = exporter_sha256.into();
        let directory = root.join("native");
        fs::create_dir(&directory).unwrap();
        up_native_profile::write_artifact(&directory.join("inventory.json"), &fixture.inventory)
            .unwrap();
        up_native_profile::write_artifact(
            &directory.join("agreement.json"),
            &matrix_table_profile_document().0,
        )
        .unwrap();
        up_native_profile::write_artifact(&directory.join("seeds.json"), &fixture.seeds).unwrap();
        up_native_profile::write_artifact(&directory.join("identities.json"), &fixture.identities)
            .unwrap();
        for name in ["stdout.log", "stderr.log"] {
            fs::write(
                directory.join(name),
                b"unit-test fixture; no exporter executed\n",
            )
            .unwrap();
        }
        for name in FILES {
            fixture
                .file_sha256
                .insert(name.into(), sha256_file(&directory.join(name)).unwrap());
            fs::set_permissions(directory.join(name), fs::Permissions::from_mode(0o444)).unwrap();
        }
        fs::set_permissions(directory, fs::Permissions::from_mode(0o555)).unwrap();
        fixture
    }

    #[cfg(test)]
    pub(super) fn fixture() -> Self {
        let inventory =
            configurable_streamer_wire_support::native_profile::matrix_native_inventory().unwrap();
        Self {
            single_type: None,
            command: command(&inventory.target.triple),
            inventory,
            seeds: matrix_seed_allocations(),
            identities: matrix_identity_declarations(),
            exporter_sha256: "a".repeat(64),
            previous_agreement_sha256: None,
            file_sha256: FILES.map(|name| (name.into(), "a".repeat(64))).into(),
        }
    }
}
