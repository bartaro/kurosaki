# Source de test élémentaire KITAQFC NROM

[English](README.md) | **Français**

**Manuel KUROSAKI** · **Manuel KITAQFC**

Ce source est destiné à être compilé ultérieurement avec l'arborescence KITAQFC Phase26, puis copié dans `crates/kurosaki-core/tests/fixtures/` comme ROM de référence produite par le compilateur.

La ROM de test actuelle `kitaqfc_nrom_smoke.nes` est générée octet par octet de manière indépendante. Elle exerce les mêmes chemins d'émulation sans nécessiter KITAQFC ni matériel physique dans cet environnement.

Commande envisagée pour un usage futur, dont les options doivent être adaptées à l'interface locale de KITAQFC si nécessaire :

```powershell
kitaqfc.exe tests\kitaqfc_smoke_src\kitaqfc_nrom_smoke.c `
  -o crates\kurosaki-core\tests\fixtures\kitaqfc_nrom_smoke_from_kitaqfc.nes `
  --mapper nrom256 `
  --debug-out crates\kurosaki-core\tests\fixtures\debug_output
```
