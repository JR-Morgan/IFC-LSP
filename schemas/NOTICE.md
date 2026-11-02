# EXPRESS schemas

These files are the unmodified EXPRESS (ISO 10303-11) long-form schemas published by
buildingSMART International for the IFC releases. They are embedded in the `ifc-lsp`
binary at compile time (`server/src/schema/embedded.rs`) and are the source of the
entity, attribute, enumeration and select tables used for hover and diagnostics.

The canonical publication location is <https://standards.buildingsmart.org/IFC/RELEASE/>,
which rejects scripted downloads. The copies here were fetched from the GitHub mirrors
listed below and are byte-identical to the published files.

| File | Release | Fetched from | sha256 |
|---|---|---|---|
| `IFC2X3_TC1.exp` | IFC2x3 TC1 | `buildingSMART/IFC4.x-development` `reference_schemas/IFC2X3_TC1.exp` | `e18a1b2c3e29f5256904c83378ccad0850f52287a8d0122d149aba4a417fe5e5` |
| `IFC4_ADD2_TC1.exp` | IFC4 ADD2 TC1 | `buildingSMART/IFC4.x-development` `reference_schemas/IFC4_ADD2_TC1.exp` | `a2704ba20a1b3d0b7d9b61d6fd37d0baa3b4996ba3e90d968a1d2ca2819d1046` |
| `IFC4X1.exp` | IFC4.1 | `buildingSMART/IFC4.x-development` `reference_schemas/IFC4x1.exp` | `0d3471e3e2fdc083321e9ffbee281ea58f48040eab1b7007bf32a5f41891c5dd` |
| `IFC4X2.exp` | IFC4.2 | `buildingSMART/IFC4.x-development` `reference_schemas/IFC4x2.exp` | `d04c43f3daf94312115e5cb816066969de0ace801446aa9c28260fc0bb490e33` |
| `IFC4X3_ADD1.exp` | IFC4.3 ADD1 | `xBimTeam/XbimEssentials` `Schemas/IFC4X3_ADD1.exp` | `75d9261a8d4636618fedb5a90d4e63bb27bdf4407641e16b9bd703e26301b7de` |
| `IFC4X3_ADD2.exp` | IFC4.3 ADD2 (ISO 16739-1:2024) | `xBimTeam/XbimEssentials` `Schemas/IFC4X3_ADD2.exp` | `f67c8762b13a099c28082061e6f16b9ef1284ceec34069792afc702725675860` |

Copyright buildingSMART International Ltd. The IFC specification is published under the
Creative Commons Attribution-NoDerivatives 4.0 International licence
(<https://creativecommons.org/licenses/by-nd/4.0/>); the files are redistributed here
unmodified with attribution. The older files additionally carry buildingSMART's historic
"all rights reserved" banner in their header comment. The Apache-2.0 licence of this
repository does not apply to the contents of this directory.
