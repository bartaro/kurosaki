# KUROSAKI

[English](README.md#english) | [日本語](README.md#japanese) | **Español**

**Abrir el manual de KUROSAKI en español**

Emulador de NES/Famicom/FDS orientado a observar la ejecución, con herramientas de línea de comandos, trazas e interfaz Python.

El proyecto está en fase de versión preliminar pública. Las API y su comportamiento pueden cambiar.

## Ejecutable para Windows

La raíz del repositorio incluye `kurosaki.exe`, compilado en modo Release para Windows x64. Es un programa de línea de comandos: para ejecutarlo no hace falta instalar Rust, Python ni .NET. Descargue el ZIP del repositorio para conservar juntos el ejecutable y los avisos de licencia. La biblioteca de ejecución de C está enlazada estáticamente; el programa utiliza DLL del sistema de Windows.

```powershell
.\kurosaki.exe --help
```

Para volver a compilar solo esta herramienta, ejecute `.\scripts\build.ps1` y añada `-Offline` si necesita trabajar sin conexión. El script copia el ejecutable a la raíz del repositorio. Esta distribución de ejecutables no incluye interfaces gráficas, módulos de extensión de Python ni DLL de la API C. Consulte el [registro de compilación del binario](BINARY_BUILD.json) y los [avisos de licencia de sus dependencias](BINARY_NOTICES.md).

El código propio del proyecto se ofrece bajo la licencia MIT, cuyo licenciante es **DAISUKE OBA**. Al redistribuirlo, incluya `LICENSE`, `LICENSE.ja`, `BINARY_NOTICES.md` y el directorio `licenses/`. Las bibliotecas de terceros conservan sus respectivos titulares y condiciones de licencia.

## Compilar el código fuente y empezar a usar la herramienta

Para recompilar, utilice una versión estable reciente de Rust. En Windows también necesita Visual Studio Build Tools con la carga de trabajo «Desarrollo para el escritorio con C++».

```powershell
.\scripts\build.ps1
.\kurosaki.exe --help
```

## Manuales y licencias

- Manual en español
- [Manual en inglés](https://bartaro.github.io/kitaq-docs/en/kurosaki.html) / [Manual en japonés](https://bartaro.github.io/kitaq-docs/kurosaki.html)
- [Archivos del manual para consultarlo sin conexión](https://github.com/bartaro/kitaq-docs)
- [Licencia](LICENSE) / [Traducción japonesa de referencia](LICENSE.ja)

La licencia del proyecto no sustituye las condiciones de terceros sobre dependencias, logotipos o marcas. Conserve los avisos adjuntos al redistribuir el software.

## Contenido de esta publicación

Se publican el núcleo del emulador, la herramienta de línea de comandos y las API de integración. Las interfaces gráficas y la dependencia de PLITA quedan para una publicación posterior y no forman parte de esta copia del código fuente.

## Desarrollo de las interfaces gráficas

KOKURA-GUI y KUROSAKI-GUI todavía no se han publicado. Su desarrollo futuro utilizará PLITA; no se utilizarán SDL-GUI ni egui-GUI. Este repositorio seguirá distribuyendo el núcleo, la herramienta de línea de comandos y las API de integración.
