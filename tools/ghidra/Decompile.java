// Headless: print decompiled C for the functions containing each address argument.
//   analyzeHeadless <proj> <name> -process LMO.exe -noanalysis -readOnly \
//     -scriptPath tools/ghidra -postScript Decompile.java 0xb4b580 [0x...]
// @category LMO
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;

public class Decompile extends GhidraScript {
    @Override
    public void run() throws Exception {
        DecompInterface ifc = new DecompInterface();
        ifc.openProgram(currentProgram);
        for (String arg : getScriptArgs()) {
            Address addr = toAddr(arg);
            Function f = getFunctionContaining(addr);
            if (f == null) {
                f = createFunction(addr, null);
            }
            if (f == null) {
                println("@@@ no function at " + arg);
                continue;
            }
            DecompileResults r = ifc.decompileFunction(f, 120, monitor);
            println("@@@ " + f.getName() + " @ " + f.getEntryPoint());
            println(r.decompileCompleted() ? r.getDecompiledFunction().getC() : "decompile failed: " + r.getErrorMessage());
        }
        ifc.dispose();
    }
}
