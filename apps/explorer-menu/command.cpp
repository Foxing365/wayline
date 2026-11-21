#include <windows.h>
#include <shobjidl.h>
#include <shlwapi.h>

#include <atomic>
#include <memory>
#include <new>
#include <string>
#include <string_view>

namespace {
constexpr CLSID SendWithWaylineId = {0x5e634822, 0x29e3, 0x4434,
    {0x8f, 0xf7, 0x67, 0xeb, 0x58, 0x47, 0x9e, 0x7f}};
constexpr size_t MaxCommandUnits = 32766; // CreateProcessW excludes the final NUL here.
HINSTANCE module = nullptr;
std::atomic<ULONG> live_objects{0};
std::atomic<ULONG> server_locks{0};

struct TaskMemory {
    void operator()(wchar_t* value) const noexcept { CoTaskMemFree(value); }
};

struct ReleaseShellItem {
    void operator()(IShellItem* item) const noexcept { item->Release(); }
};

HRESULT DesktopPath(std::wstring& path) {
    path.resize(MaxCommandUnits);
    const DWORD size = GetModuleFileNameW(module, path.data(), static_cast<DWORD>(path.size()));
    if (!size) return HRESULT_FROM_WIN32(GetLastError());
    if (size >= path.size()) return HRESULT_FROM_WIN32(ERROR_FILENAME_EXCED_RANGE);
    path.resize(size);
    const auto separator = path.rfind(L'\\');
    if (separator == std::wstring::npos) return HRESULT_FROM_WIN32(ERROR_BAD_PATHNAME);
    path.replace(separator + 1, std::wstring::npos, L"wayline-desktop.exe");
    return S_OK;
}

// Double backslashes preceding a quote, including trailing backslashes before
// our closing quote. This preserves every path in the Windows argv parser.
bool AppendArgument(std::wstring& command_line, std::wstring_view argument) {
    if (command_line.size() + argument.size() + 3 > MaxCommandUnits) return false;
    if (!command_line.empty()) command_line.push_back(L' ');
    command_line.push_back(L'"');
    size_t backslashes = 0;
    for (const wchar_t character : argument) {
        if (character == L'\\') {
            ++backslashes;
            continue;
        }
        command_line.append(character == L'"' ? backslashes * 2 + 1 : backslashes, L'\\');
        backslashes = 0;
        command_line.push_back(character);
    }
    command_line.append(backslashes * 2, L'\\');
    command_line.push_back(L'"');
    return command_line.size() <= MaxCommandUnits;
}

HRESULT LaunchSelection(IShellItemArray* selection) {
    if (!selection) return E_INVALIDARG;
    DWORD count = 0;
    HRESULT status = selection->GetCount(&count);
    if (FAILED(status)) return status;
    if (!count) return E_INVALIDARG;

    std::wstring desktop;
    status = DesktopPath(desktop);
    if (FAILED(status)) return status;
    std::wstring command_line;
    if (!AppendArgument(command_line, desktop)) return HRESULT_FROM_WIN32(ERROR_FILENAME_EXCED_RANGE);
    command_line += L" --send --";
    for (DWORD i = 0; i < count; ++i) {
        IShellItem* item = nullptr;
        status = selection->GetItemAt(i, &item);
        if (FAILED(status)) return status;
        const std::unique_ptr<IShellItem, ReleaseShellItem> selected(item);
        PWSTR path = nullptr;
        status = selected->GetDisplayName(SIGDN_FILESYSPATH, &path);
        if (FAILED(status)) return status;
        const std::unique_ptr<wchar_t, TaskMemory> filesystem_path(path);
        if (!path || !*path) return E_INVALIDARG;
        if (!AppendArgument(command_line, path)) return HRESULT_FROM_WIN32(ERROR_FILENAME_EXCED_RANGE);
    }

    // Resolve only the Wayline executable located beside the extension. No
    // shell verb, PATH search, console, network I/O, or elevated token is used.
    STARTUPINFOW startup{};
    startup.cb = sizeof(startup);
    PROCESS_INFORMATION process{};
    if (!CreateProcessW(desktop.c_str(), command_line.data(), nullptr, nullptr, FALSE,
                       0, nullptr, nullptr, &startup, &process)) {
        return HRESULT_FROM_WIN32(GetLastError());
    }
    CloseHandle(process.hThread);
    CloseHandle(process.hProcess);
    return S_OK;
}

template <typename Interface>
class ComObject : public Interface {
public:
    ComObject() noexcept { ++live_objects; }

    ULONG STDMETHODCALLTYPE AddRef() final { return ++references_; }

    ULONG STDMETHODCALLTYPE Release() final {
        const ULONG remaining = --references_;
        if (!remaining) delete this;
        return remaining;
    }

protected:
    virtual ~ComObject() { --live_objects; }

private:
    std::atomic<ULONG> references_{1};
};

class SendCommand final : public ComObject<IExplorerCommand> {
public:
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** result) override {
        if (!result) return E_POINTER;
        *result = nullptr;
        if (iid != IID_IUnknown && iid != IID_IExplorerCommand) return E_NOINTERFACE;
        *result = static_cast<IExplorerCommand*>(this);
        AddRef();
        return S_OK;
    }

    HRESULT STDMETHODCALLTYPE GetTitle(IShellItemArray*, PWSTR* title) override {
        if (!title) return E_POINTER;
        LPCWSTR text = L"Send via Wayline";
        const auto language = GetUserDefaultUILanguage();
        if (language == 0x0404 || language == 0x0c04 || language == 0x1404) {
            text = L"透過 Wayline 傳送";
        } else if (PRIMARYLANGID(language) == LANG_CHINESE) {
            text = L"通过 Wayline 发送";
        }
        return SHStrDupW(text, title);
    }

    HRESULT STDMETHODCALLTYPE GetIcon(IShellItemArray*, PWSTR* icon) override {
        if (!icon) return E_POINTER;
        *icon = nullptr;
        try {
            std::wstring desktop;
            const HRESULT status = DesktopPath(desktop);
            if (FAILED(status)) return status;
            desktop = L'"' + desktop + L"\",0";
            return SHStrDupW(desktop.c_str(), icon);
        } catch (const std::bad_alloc&) {
            return E_OUTOFMEMORY;
        }
    }

    HRESULT STDMETHODCALLTYPE GetToolTip(IShellItemArray*, PWSTR* tooltip) override {
        if (tooltip) *tooltip = nullptr;
        return E_NOTIMPL;
    }

    HRESULT STDMETHODCALLTYPE GetCanonicalName(GUID* guid) override {
        if (!guid) return E_POINTER;
        *guid = SendWithWaylineId;
        return S_OK;
    }

    HRESULT STDMETHODCALLTYPE GetState(IShellItemArray*, BOOL, EXPCMDSTATE* state) override {
        if (!state) return E_POINTER;
        // Explorer can query the command before furnishing the selection.
        // The package limits it to file/folder menus; Invoke validates paths.
        *state = ECS_ENABLED;
        return S_OK;
    }

    HRESULT STDMETHODCALLTYPE Invoke(IShellItemArray* selection, IBindCtx*) override {
        try {
            return LaunchSelection(selection);
        } catch (const std::bad_alloc&) {
            return E_OUTOFMEMORY;
        }
    }

    HRESULT STDMETHODCALLTYPE GetFlags(EXPCMDFLAGS* flags) override {
        if (!flags) return E_POINTER;
        *flags = ECF_DEFAULT;
        return S_OK;
    }

    HRESULT STDMETHODCALLTYPE EnumSubCommands(IEnumExplorerCommand** commands) override {
        if (commands) *commands = nullptr;
        return E_NOTIMPL;
    }
};

class CommandFactory final : public ComObject<IClassFactory> {
public:
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** result) override {
        if (!result) return E_POINTER;
        *result = nullptr;
        if (iid != IID_IUnknown && iid != IID_IClassFactory) return E_NOINTERFACE;
        *result = static_cast<IClassFactory*>(this);
        AddRef();
        return S_OK;
    }

    HRESULT STDMETHODCALLTYPE CreateInstance(IUnknown* outer, REFIID iid, void** result) override {
        if (!result) return E_POINTER;
        *result = nullptr;
        if (outer) return CLASS_E_NOAGGREGATION;
        const auto command = new (std::nothrow) SendCommand;
        if (!command) return E_OUTOFMEMORY;
        const HRESULT status = command->QueryInterface(iid, result);
        command->Release();
        return status;
    }

    HRESULT STDMETHODCALLTYPE LockServer(BOOL lock) override {
        if (lock) {
            ++server_locks;
        } else {
            ULONG held = server_locks.load();
            while (held && !server_locks.compare_exchange_weak(held, held - 1)) {}
        }
        return S_OK;
    }
};
} // namespace

extern "C" BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) module = instance;
    return TRUE;
}

extern "C" HRESULT STDAPICALLTYPE DllGetClassObject(
    REFCLSID clsid, REFIID iid, void** result) {
    if (!result) return E_POINTER;
    *result = nullptr;
    if (clsid != SendWithWaylineId) return CLASS_E_CLASSNOTAVAILABLE;
    const auto factory = new (std::nothrow) CommandFactory;
    if (!factory) return E_OUTOFMEMORY;
    const HRESULT status = factory->QueryInterface(iid, result);
    factory->Release();
    return status;
}

extern "C" HRESULT STDAPICALLTYPE DllCanUnloadNow() {
    return live_objects.load() || server_locks.load() ? S_FALSE : S_OK;
}
