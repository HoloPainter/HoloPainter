# Model import fixtures

`triangle_ascii.fbx` is a hand-authored minimal FBX 7.4 ASCII fixture created for HoloPainter.
It is intentionally small enough to audit and contains one centimeter-scale triangle, UV0,
normals, and one mesh node. Unit tests derive additional in-memory fixtures from it for polygon,
attribute-index, transform, instance, and material-assignment cases.

The fixture is project test data and may be used under the same terms as HoloPainter.
