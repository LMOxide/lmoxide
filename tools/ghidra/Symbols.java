// Headless: list symbols whose full name matches a regex; for vftables also print the first
// N slot targets.   args: <regex> [slots=0]
// @category LMO
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Symbol;
import java.util.regex.Pattern;

public class Symbols extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        Pattern p = Pattern.compile(a[0]);
        int slots = a.length > 1 ? Integer.parseInt(a[1]) : 0;
        for (Symbol s : currentProgram.getSymbolTable().getAllSymbols(true)) {
            String name = s.getName(true);
            if (!p.matcher(name).find()) continue;
            println(s.getAddress() + "  " + name);
            for (int i = 0; i < slots && name.endsWith("vftable"); i++) {
                Address slot = s.getAddress().add(4L * i);
                Address t = toAddr(getInt(slot) & 0xffffffffL);
                Function f = getFunctionAt(t);
                println(String.format("    [%2d] +0x%02x %s %s", i, 4 * i, t, f == null ? "" : f.getName()));
            }
        }
    }
}
