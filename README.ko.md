# KUROSAKI

<!-- readme-language-links:start -->
[English](README.md#english) | [日本語](README.md#japanese) | **한국어** | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | [Français](README.fr.md) | [Español](README.es.md) | [Deutsch](README.de.md)
<!-- readme-language-links:end -->

**[KUROSAKI · HTML 설명서](https://bartaro.github.io/kitaq-docs/ko/kurosaki.html)**

NES/Famicom/FDS의 실행 상태를 관찰하는 에뮬레이터입니다. 명령줄 도구, 트레이스, Python 인터페이스를 제공합니다.

공개 미리 보기 버전으로, API와 동작이 변경될 수 있습니다.

## Windows 실행 파일

저장소 최상위 폴더에 Windows x64용 Release 빌드인 `kurosaki.exe`가 있습니다. 명령줄에서 사용하는 프로그램이며, 실행할 때 Rust, Python, .NET을 별도로 설치할 필요는 없습니다. 실행 파일과 라이선스 고지를 함께 받을 수 있도록 저장소를 ZIP으로 내려받으세요. 네이티브 C 런타임은 정적으로 연결되어 있으며, Windows 시스템 DLL을 사용합니다.

```powershell
.\kurosaki.exe --help
```

이 명령줄 도구만 다시 빌드하려면 `.\scripts\build.ps1`을 실행하세요. 필요하면 `-Offline`을 추가할 수 있습니다. 스크립트가 실행 파일을 저장소 최상위 폴더로 복사합니다. GUI, Python 확장 모듈, C API DLL은 이 실행 파일 배포에 포함되지 않습니다. 자세한 내용은 [바이너리 빌드 기록](BINARY_BUILD.json)과 [바이너리 의존성 라이선스 고지](BINARY_NOTICES.md)를 확인하세요.

프로젝트 자체 코드의 라이선스 제공자는 **DAISUKE OBA**이며, MIT 라이선스로 배포합니다. 재배포할 때는 `LICENSE`, `LICENSE.ja`, `BINARY_NOTICES.md`, `licenses/` 폴더를 함께 포함하세요. 외부 라이브러리에는 각 저작권자의 라이선스 조건이 적용됩니다.

## 직접 빌드하고 실행하기

다시 빌드하려면 최신 안정 버전의 Rust 도구 모음을 사용하세요. Windows에서는 Visual Studio Build Tools와 ‘C++를 사용한 데스크톱 개발’ 워크로드도 필요합니다.

```powershell
.\scripts\build.ps1
.\kurosaki.exe --help
```

<!-- current-nametable-mapping -->
## 네임테이블 매핑

AxROM은 하위·상위 단일 화면 선택을 지원합니다. FME7은 수직·수평 미러링과 두 단일 화면 모드를 지원합니다. MMC5는 CIRAM 페이지 두 개를 사용하는 16가지 배치를 모두 지원하지만 ExRAM과 채우기 렌더링은 지원하지 않습니다. [모드 표와 실행 예](https://bartaro.github.io/kitaq-docs/ko/kitaqfc.html#api-__mirroring_set).

## 설명서와 라이선스

- [KUROSAKI · HTML 설명서](https://bartaro.github.io/kitaq-docs/ko/kurosaki.html)
- [영어 설명서](https://bartaro.github.io/kitaq-docs/en/kurosaki.html) / [일본어 설명서](https://bartaro.github.io/kitaq-docs/kurosaki.html)
- [오프라인 열람용 설명서 소스](https://github.com/bartaro/kitaq-docs)
- [라이선스](LICENSE) / [일본어 참고 번역](LICENSE.ja)

프로젝트 라이선스가 외부 의존성, 로고, 상표에 관한 제3자의 조건을 대신하지는 않습니다. 재배포할 때 동봉된 고지를 유지하세요.
