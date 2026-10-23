// Headless: list references to each address argument (code and data), with the containing
// function, and for data refs (e.g. vtable slots) the references to that data in turn.
// @category LMO
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Reference;

public class Xrefs extends GhidraScript {
    private String where(Address a) {
        Function f = getFunctionContaining(a);
        return f == null ? "(no function)" : f.getName() + " @ " + f.getEntryPoint();
    }

    @Override
    public void run() throws Exception {
        for (String arg : getScriptArgs()) {
            Address target = toAddr(arg);
            println("@@@ refs to " + target + " [" + where(target) + "]");
            for (Reference r : getReferencesTo(target)) {
                Address from = r.getFromAddress();
                println("  " + r.getReferenceType() + " from " + from + "  in " + where(from));
                if (getFunctionContaining(from) == null) {
                    for (Reference r2 : getReferencesTo(from)) {
                        println("      <- " + r2.getReferenceType() + " from " + r2.getFromAddress() + "  in " + where(r2.getFromAddress()));
                    }
                }
            }
        }
    }
}
