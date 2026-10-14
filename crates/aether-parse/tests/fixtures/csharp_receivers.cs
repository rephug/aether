using Acme.Io;

namespace Acme.Billing;

public class Ledger
{
    private Pointer cursor;
    private Audit deposit;
    private Action<int> _onPost;

    public string Name { get; set; }

    public event EventHandler Changed;

    public Entry Post(Account account, Event e)
    {
        for (Cursor cursor = account.Cursor(); cursor.HasNext(); cursor.Advance())
        {
            cursor.Mark();
        }
        if (e is Deposit deposit && deposit.IsLarge())
        {
            deposit.Review();
        }
        using (Session session = Open())
        {
            session.Flush();
        }
        return new Entry(NextNumber(), this.Label());
    }

    public static Builder Create()
    {
        return new Builder();
    }

    public void Configure()
    {
        Ledger.Builder builder = Ledger.Create();
        builder.Limit(5);
    }

    public void Route(object e)
    {
        switch (e)
        {
            case Withdrawal withdrawal:
                withdrawal.Cancel();
                break;
        }
    }

    public decimal Amount(object e) => e switch
    {
        Deposit deposit => deposit.Total(),
        _ => 0,
    };

    [Obsolete]
    public void Reset() { }

    public string Shout() => Name.ToUpper();

    public void Guard(Account account)
    {
        if (account is null) throw new ArgumentNullException(nameof(account));
    }

    public (bool Success, Entry Result) Batch(params Entry[] entries) => (true, null);

    public void Raise()
    {
        Changed?.Invoke(this, EventArgs.Empty);
        Changed(this, EventArgs.Empty);
    }

    public T Make<T>() where T : new() => new T();

    public void Touch<TEntity>(TEntity entity) => entity.Validate();

    public void Safe()
    {
        try
        {
            Open();
        }
        catch (LedgerException failure)
        {
            failure.Report();
        }
    }

    public void Drain(object e)
    {
        while (e is Deposit deposit)
        {
            deposit.Settle();
            e = null;
        }
    }

    public void Fire()
    {
        _onPost(1);
        Validate(1);
    }

    public void Render(Acme.Model.Node model)
    {
        var copy = new Acme.Model.Node();
    }

    public class Node { }

    private static void Validate(int value) { }

    public class Builder
    {
        public Builder Limit(int value)
        {
            Validate(value);
            return this;
        }
    }

    private Session Open() => null;

    private int NextNumber() => 1;

    private string Label() => "";
}

public class Svc(IRepo repo)
{
    public void Run() => repo.Save();
}

public class Scene
{
    public void Add(Builder builder) { }
}

public class Result
{
    public override string ToString() => "ok";
}

public class Result<T>
{
    public override string ToString() => "ok";
}

public class Overloads
{
    public void Find(string key) => Find(key, 10);

    public void Find(string key, int limit) => Log(key);

    private void Log(string key) { }
}

public class Client
{
#if NET8_0_OR_GREATER
    private ILogger Log { get; }
#else
    private LegacyLogger Log { get; }
#endif

    public void Send() => Log.Info("sent");
}

public class Outer
{
#if DEBUG
    private static void Trace(string message) { }
#endif

    public class Inner
    {
        public void Run() => Trace("run");
    }
}

public class Report
{
    public void Add(Payment.Status status) { }

    public void Order() => new CreateOrder.Command().Send();
}

public class Task { }

public class Board : Task
{
    public void Add(Task task) { }
}
