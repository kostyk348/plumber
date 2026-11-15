# plumber — короткие обёртки для fish. Ставится в ~/.config/fish/conf.d/plumber.fish

if type -q plumb
    function pl --description 'plumber: отправить текст по правилам'
        plumb send $argv
    end
    function plf --description 'plumber: открыть файл на строке'
        set -l line 1
        if set -q argv[2]
            set line $argv[2]
        end
        plumb send "$argv[1]:$line"
    end
    function pls --description 'plumber: состояние демона и читателей'
        plumber-session status
    end
end
