# Código fuente de la prueba básica NROM de KITAQFC

<!-- readme-language-links:start -->
[English](README.md) | [日本語](README.ja.md) | [한국어](README.ko.md) | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | [Français](README.fr.md) | **Español** | [Deutsch](README.de.md)
<!-- readme-language-links:end -->

**[KUROSAKI · Manual HTML](https://bartaro.github.io/kitaq-docs/es/kurosaki.html) · [KITAQFC · Manual HTML](https://bartaro.github.io/kitaq-docs/es/kitaqfc.html)**

Está previsto compilar este código con el árbol de desarrollo Phase26 de KITAQFC y copiar la ROM resultante a `crates/kurosaki-core/tests/fixtures/` como referencia de prueba. Esto no significa que ya se haya verificado su compilación.

La ROM de prueba actual, `kitaqfc_nrom_smoke.nes`, se construyó con una secuencia de bytes generada de forma independiente, sin código de juegos comerciales. Permite comprobar las mismas rutas del emulador aunque este entorno no disponga de KITAQFC ni de hardware real.

El siguiente comando es un borrador para una futura compilación. Antes de ejecutarlo, adapte las opciones a la versión de KITAQFC instalada; no es un procedimiento ya verificado para la CLI actual.

```powershell
kitaqfc.exe tests\kitaqfc_smoke_src\kitaqfc_nrom_smoke.c `
  -o crates\kurosaki-core\tests\fixtures\kitaqfc_nrom_smoke_from_kitaqfc.nes `
  --mapper nrom256 `
  --debug-out crates\kurosaki-core\tests\fixtures\debug_output
```
