# Microsoft Store release

HoloPainter 3D is distributed as one free MSIX package. The durable Microsoft
Store add-on with product ID `pro_unlock` permanently enables HoloPainter Pro.
PSD export is the first Pro-only feature.

## Partner Center setup

Create an add-on associated with HoloPainter 3D using these values:

- Product type: Durable
- Product lifetime: Forever
- Product ID: `pro_unlock`
- In-app offer token: `pro_unlock` (case-sensitive; must exactly match the app)
- Packages: none
- Visibility: purchasable from the app; do not give the add-on a separate Store presence
- Markets: the same markets as the parent app

The parent app itself must be free and must not be configured as a trial.

Before building, copy these case-sensitive values from **Product management →
Product identity** in Partner Center:

- Package/Identity/Name
- Package/Identity/Publisher
- Package/Properties/PublisherDisplayName

## MSIX icon assets

`packaging/msix/Assets/` contains generated, committed PNG assets used only by
MSIX. The Windows executable continues to use `assets/app_icon/app_icon.ico`.
When `assets/app_icon/app_icon.svg` changes, regenerate and review the PNGs:

```powershell
.\scripts\generate-msix-assets.ps1 `
  -InputSvg .\assets\app_icon\app_icon.svg
```

The MSIX build copies the committed asset directory as-is. It does not run
Inkscape or regenerate icons.

## Build

Run from PowerShell:

```powershell
.\scripts\build-msix.ps1 `
  -IdentityName '<Package Identity Name>' `
  -Publisher '<Publisher>' `
  -PublisherDisplayName '<Publisher display name>'
```

The script reads the three-component package version from `Cargo.toml` and
appends `.0` for the four-component MSIX identity version. For example, Cargo
version `1.0.2` produces MSIX version `1.0.2.0`.

The script validates and copies all committed package icons, builds the x64
release executable, verifies the packed contents, and writes the unsigned Store
package below `target/msix/`. Microsoft signs the package after certification.
For sideload installation, sign the package with a trusted test or code-signing
certificate whose subject exactly matches the manifest Publisher.

The fourth component of a Store package version must remain zero. Never reuse
the same package version for different package contents.

## Purchase testing

`Windows.Services.Store` has no local license simulator. Publish a hidden,
certified package and add-on, install the app from Microsoft Store once with a
tester account, and then test:

1. Free account sees the Pro Unlock dialog instead of PSD export.
2. The Store returns a localized price.
3. Purchase cancellation keeps PSD export locked.
4. A successful purchase unlocks PSD export after license revalidation.
5. Restart and offline launch retain a cached durable entitlement.
6. A second device with the purchasing account restores the entitlement.
7. An account without the add-on remains Free.

Also run the Windows App Certification Kit and manually verify the Start menu,
taskbar icon, `.holopaint` activation, project save/load, image export, PSD
export after purchase, GPU rendering, and tablet input.
