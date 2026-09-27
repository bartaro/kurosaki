# KUROSAKI

<!-- readme-language-links:start -->
[English](README.md#english) | [日本語](README.md#japanese) | [한국어](README.ko.md) | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | **Français** | [Español](README.es.md) | [Deutsch](README.de.md)
<!-- readme-language-links:end -->

**[KUROSAKI · Manuel HTML](https://bartaro.github.io/kitaq-docs/fr/kurosaki.html)**

Émulateur d’observation NES/Famicom/FDS avec interface en ligne de commande, traces et interface Python.

Version publique préliminaire : les API et leur comportement peuvent évoluer.

## Exécutable pour Windows

La racine du dépôt contient `kurosaki.exe`, compilé en mode Release pour Windows x64. Il s'agit d'un programme en ligne de commande. Son exécution ne nécessite ni Rust, ni Python, ni .NET. Téléchargez l'archive ZIP du dépôt pour conserver ensemble l'exécutable et les mentions de licence. La bibliothèque d'exécution C native est liée statiquement ; le programme utilise des DLL système de Windows.

```powershell
.\kurosaki.exe --help
```

Pour reconstruire cet outil seul, utilisez `.\scripts\build.ps1`, avec l'option `-Offline` au besoin. Le script copie l'exécutable à la racine du dépôt. Les interfaces graphiques, modules d'extension Python et DLL de l'API C ne font pas partie de cette distribution d'exécutables. Consultez le [compte rendu de compilation binaire](BINARY_BUILD.json) et les [mentions relatives aux dépendances de l'exécutable](BINARY_NOTICES.md).

Le code propre au projet est proposé sous licence MIT par **DAISUKE OBA**. Lors d'une redistribution, joignez `LICENSE`, `LICENSE.ja`, `BINARY_NOTICES.md` et le dossier `licenses/`. Les bibliothèques tierces conservent leurs propres titulaires de droits et conditions.

## Compiler et commencer à utiliser l'outil

Pour recompiler, utilisez une version stable actuelle de Rust. Sous Windows, installez Visual Studio Build Tools avec la charge de travail de développement Desktop en C++.

```powershell
.\scripts\build.ps1
.\kurosaki.exe --help
```

<!-- current-nametable-mapping -->
## Mappage des tables de noms

AxROM permet de choisir la page basse ou haute en mode écran unique. FME7 prend en charge les modes vertical, horizontal et les deux modes à écran unique. MMC5 gère les seize dispositions utilisant les deux pages CIRAM ; le rendu ExRAM et le remplissage ne sont pas pris en charge. [Table des modes et exemples exécutés](https://bartaro.github.io/kitaq-docs/fr/kitaqfc.html#api-__mirroring_set).

## Manuels et licences

- [KUROSAKI · Manuel HTML](https://bartaro.github.io/kitaq-docs/fr/kurosaki.html)
- [Manuel en anglais](https://bartaro.github.io/kitaq-docs/en/kurosaki.html) / [Manuel en japonais](https://bartaro.github.io/kitaq-docs/kurosaki.html)
- [Sources du manuel pour lecture hors connexion](https://github.com/bartaro/kitaq-docs)
- [Licence](LICENSE) / [Traduction japonaise à titre de référence](LICENSE.ja)

La licence du projet ne remplace pas les conditions des tiers relatives aux dépendances, logos ou marques. Conservez les mentions jointes lors de la redistribution.
